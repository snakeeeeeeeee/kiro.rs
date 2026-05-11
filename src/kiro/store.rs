use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use parking_lot::Mutex;
use rusqlite::{Connection, params};

use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::settings::{CredentialPolicy, RuntimeSettings};
use crate::model::config::Config;

#[derive(Clone)]
pub struct KiroStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StoredCredential {
    pub credentials: KiroCredentials,
    pub policy: CredentialPolicy,
    pub failure_count: u32,
    pub refresh_failure_count: u32,
    pub success_count: u64,
    pub last_used_at: Option<String>,
    pub disabled_reason: Option<String>,
}

impl KiroStore {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建数据库目录失败: {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("打开 SQLite 数据库失败: {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn default_path_for_config(config_path: &Path) -> PathBuf {
        config_path
            .parent()
            .map(|p| p.join("kiro-rs.db"))
            .unwrap_or_else(|| PathBuf::from("kiro-rs.db"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS credentials (
                id INTEGER PRIMARY KEY,
                data_json TEXT NOT NULL,
                max_concurrent_override INTEGER NULL,
                rpm_override INTEGER NULL,
                failure_count INTEGER NOT NULL DEFAULT 0,
                refresh_failure_count INTEGER NOT NULL DEFAULT 0,
                success_count INTEGER NOT NULL DEFAULT 0,
                last_used_at TEXT NULL,
                disabled_reason TEXT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS runtime_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    pub fn is_empty(&self) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM credentials", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    pub fn initialize_runtime_settings(&self, settings: &RuntimeSettings) -> anyhow::Result<()> {
        settings.validate()?;
        let conn = self.conn.lock();
        for (key, value) in runtime_settings_pairs(settings)? {
            conn.execute(
                r#"
                INSERT INTO runtime_settings (key, value, updated_at)
                VALUES (?1, ?2, CURRENT_TIMESTAMP)
                ON CONFLICT(key) DO NOTHING
                "#,
                params![key, value],
            )?;
        }
        Ok(())
    }

    pub fn load_runtime_settings(
        &self,
        defaults: &RuntimeSettings,
    ) -> anyhow::Result<RuntimeSettings> {
        let conn = self.conn.lock();
        let mut settings = defaults.clone();
        let mut stmt = conn.prepare("SELECT key, value FROM runtime_settings")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (key, value) = row?;
            apply_runtime_setting(&mut settings, &key, &value)?;
        }
        settings.load_balancing_mode =
            crate::kiro::settings::normalize_load_balancing_mode(&settings.load_balancing_mode);
        settings.validate()?;
        Ok(settings)
    }

    pub fn save_runtime_settings(&self, settings: &RuntimeSettings) -> anyhow::Result<()> {
        settings.validate()?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for (key, value) in runtime_settings_pairs(settings)? {
            tx.execute(
                r#"
                INSERT INTO runtime_settings (key, value, updated_at)
                VALUES (?1, ?2, CURRENT_TIMESTAMP)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP
                "#,
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn import_credentials_if_empty(
        &self,
        credentials: &[KiroCredentials],
        config: &Config,
    ) -> anyhow::Result<bool> {
        if !self.is_empty()? || credentials.is_empty() {
            return Ok(false);
        }
        let mut credentials = credentials.to_vec();
        let mut next_id = credentials.iter().filter_map(|c| c.id).max().unwrap_or(0) + 1;
        for credential in &mut credentials {
            credential.canonicalize_auth_method();
            if credential.id.is_none() {
                credential.id = Some(next_id);
                next_id += 1;
            }
            if credential.machine_id.is_none() {
                credential.machine_id =
                    Some(machine_id::generate_from_credentials(credential, config));
            }
        }

        let stored = credentials
            .into_iter()
            .map(|credentials| StoredCredential {
                disabled_reason: if credentials.disabled {
                    Some("Manual".to_string())
                } else {
                    None
                },
                credentials,
                policy: CredentialPolicy::default(),
                failure_count: 0,
                refresh_failure_count: 0,
                success_count: 0,
                last_used_at: None,
            })
            .collect::<Vec<_>>();
        self.replace_all_credentials(&stored)?;
        Ok(true)
    }

    pub fn load_credentials(&self) -> anyhow::Result<Vec<StoredCredential>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT data_json, max_concurrent_override, rpm_override, failure_count,
                   refresh_failure_count, success_count, last_used_at, disabled_reason
            FROM credentials
            ORDER BY COALESCE(json_extract(data_json, '$.priority'), 0), id
            "#,
        )?;
        let rows = stmt.query_map([], stored_credential_from_row)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn replace_all_credentials(&self, credentials: &[StoredCredential]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM credentials", [])?;
        for entry in credentials {
            insert_or_replace_stored_credential_tx(&tx, entry)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_policy(&self, id: u64, policy: &CredentialPolicy) -> anyhow::Result<()> {
        policy.validate()?;
        let conn = self.conn.lock();
        let updated = conn.execute(
            r#"
            UPDATE credentials
            SET max_concurrent_override = ?2,
                rpm_override = ?3,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?1
            "#,
            params![
                id,
                policy.max_concurrent_override.map(|v| v as i64),
                policy.rpm_override.map(|v| v as i64)
            ],
        )?;
        if updated == 0 {
            anyhow::bail!("凭据不存在: {}", id);
        }
        Ok(())
    }
}

fn runtime_settings_pairs(
    settings: &RuntimeSettings,
) -> anyhow::Result<Vec<(&'static str, String)>> {
    settings.validate()?;
    Ok(vec![
        (
            "globalMaxConcurrent",
            settings.global_max_concurrent.to_string(),
        ),
        (
            "perAccountDefaultMaxConcurrent",
            settings.per_account_default_max_concurrent.to_string(),
        ),
        ("queueMaxSize", settings.queue_max_size.to_string()),
        ("queueTimeoutMs", settings.queue_timeout_ms.to_string()),
        (
            "perAccountDefaultRpm",
            settings.per_account_default_rpm.to_string(),
        ),
        ("globalRpm", settings.global_rpm.to_string()),
        (
            "rateLimitCooldownMs",
            settings.rate_limit_cooldown_ms.to_string(),
        ),
        (
            "transientCooldownMs",
            settings.transient_cooldown_ms.to_string(),
        ),
        ("maxRetryAccounts", settings.max_retry_accounts.to_string()),
        (
            "modelCapacityCooldownMs",
            settings.model_capacity_cooldown_ms.to_string(),
        ),
        (
            "tokenAutoRefreshEnabled",
            settings.token_auto_refresh_enabled.to_string(),
        ),
        (
            "tokenAutoRefreshIntervalSecs",
            settings.token_auto_refresh_interval_secs.to_string(),
        ),
        (
            "tokenAutoRefreshWindowSecs",
            settings.token_auto_refresh_window_secs.to_string(),
        ),
        ("loadBalancingMode", settings.load_balancing_mode.clone()),
        (
            "virtualCacheUsageEnabled",
            settings.virtual_cache_usage_enabled.to_string(),
        ),
        (
            "virtualCacheDefaultTtl",
            settings.virtual_cache_default_ttl.clone(),
        ),
        (
            "virtualCacheUncachedInputTokens",
            settings.virtual_cache_uncached_input_tokens.to_string(),
        ),
        (
            "virtualCacheInputMode",
            settings.virtual_cache_input_mode.clone(),
        ),
        (
            "virtualCacheMinInputTokens",
            settings.virtual_cache_min_input_tokens.to_string(),
        ),
        (
            "virtualCacheMaxInputTokens",
            settings.virtual_cache_max_input_tokens.to_string(),
        ),
        (
            "virtualCacheWarmupTokens",
            settings.virtual_cache_warmup_tokens.to_string(),
        ),
        (
            "virtualCacheMinCreationTokens",
            settings.virtual_cache_min_creation_tokens.to_string(),
        ),
        (
            "virtualCacheMaxCreationTokens",
            settings.virtual_cache_max_creation_tokens.to_string(),
        ),
        (
            "virtualCacheCreationMode",
            settings.virtual_cache_creation_mode.clone(),
        ),
        (
            "virtualCacheCreationJitterRatio",
            settings.virtual_cache_creation_jitter_ratio.to_string(),
        ),
        (
            "virtualCacheBurstEveryTurns",
            settings.virtual_cache_burst_every_turns.to_string(),
        ),
        (
            "virtualCacheBurstMinTokens",
            settings.virtual_cache_burst_min_tokens.to_string(),
        ),
        (
            "virtualCacheBurstMaxTokens",
            settings.virtual_cache_burst_max_tokens.to_string(),
        ),
        (
            "virtualCacheFallbackScope",
            settings.virtual_cache_fallback_scope.clone(),
        ),
    ])
}

fn apply_runtime_setting(
    settings: &mut RuntimeSettings,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    match key {
        "globalMaxConcurrent" => settings.global_max_concurrent = parse_usize(key, value)?,
        "perAccountDefaultMaxConcurrent" => {
            settings.per_account_default_max_concurrent = parse_usize(key, value)?
        }
        "queueMaxSize" => settings.queue_max_size = parse_usize(key, value)?,
        "queueTimeoutMs" => settings.queue_timeout_ms = parse_u64(key, value)?,
        "perAccountDefaultRpm" => settings.per_account_default_rpm = parse_u32(key, value)?,
        "globalRpm" => settings.global_rpm = parse_u32(key, value)?,
        "rateLimitCooldownMs" => settings.rate_limit_cooldown_ms = parse_u64(key, value)?,
        "transientCooldownMs" => settings.transient_cooldown_ms = parse_u64(key, value)?,
        "maxRetryAccounts" => settings.max_retry_accounts = parse_usize(key, value)?,
        "modelCapacityCooldownMs" => settings.model_capacity_cooldown_ms = parse_u64(key, value)?,
        "tokenAutoRefreshEnabled" => settings.token_auto_refresh_enabled = parse_bool(key, value)?,
        "tokenAutoRefreshIntervalSecs" => {
            settings.token_auto_refresh_interval_secs = parse_u64(key, value)?
        }
        "tokenAutoRefreshWindowSecs" => {
            settings.token_auto_refresh_window_secs = parse_u64(key, value)?
        }
        "loadBalancingMode" => settings.load_balancing_mode = value.to_string(),
        "virtualCacheUsageEnabled" => {
            settings.virtual_cache_usage_enabled = parse_bool(key, value)?
        }
        "virtualCacheDefaultTtl" => {
            settings.virtual_cache_default_ttl =
                crate::kiro::settings::normalize_virtual_cache_ttl(value)
        }
        "virtualCacheUncachedInputTokens" => {
            settings.virtual_cache_uncached_input_tokens = parse_u32(key, value)?
        }
        "virtualCacheInputMode" => {
            settings.virtual_cache_input_mode =
                crate::kiro::settings::normalize_virtual_cache_input_mode(value)
        }
        "virtualCacheMinInputTokens" => {
            settings.virtual_cache_min_input_tokens = parse_u32(key, value)?
        }
        "virtualCacheMaxInputTokens" => {
            settings.virtual_cache_max_input_tokens = parse_u32(key, value)?
        }
        "virtualCacheWarmupTokens" => settings.virtual_cache_warmup_tokens = parse_u32(key, value)?,
        "virtualCacheMinCreationTokens" => {
            settings.virtual_cache_min_creation_tokens = parse_u32(key, value)?
        }
        "virtualCacheMaxCreationTokens" => {
            settings.virtual_cache_max_creation_tokens = parse_u32(key, value)?
        }
        "virtualCacheCreationMode" => {
            settings.virtual_cache_creation_mode =
                crate::kiro::settings::normalize_virtual_cache_creation_mode(value)
        }
        "virtualCacheCreationJitterRatio" => {
            settings.virtual_cache_creation_jitter_ratio = parse_f64(key, value)?
        }
        "virtualCacheBurstEveryTurns" => {
            settings.virtual_cache_burst_every_turns = parse_u32(key, value)?
        }
        "virtualCacheBurstMinTokens" => {
            settings.virtual_cache_burst_min_tokens = parse_u32(key, value)?
        }
        "virtualCacheBurstMaxTokens" => {
            settings.virtual_cache_burst_max_tokens = parse_u32(key, value)?
        }
        "virtualCacheFallbackScope" => {
            settings.virtual_cache_fallback_scope =
                crate::kiro::settings::normalize_virtual_cache_fallback_scope(value)
        }
        _ => {}
    }
    Ok(())
}

fn parse_usize(key: &str, value: &str) -> anyhow::Result<usize> {
    value
        .parse::<usize>()
        .with_context(|| format!("runtime setting {} 不是有效整数", key))
}

fn parse_u32(key: &str, value: &str) -> anyhow::Result<u32> {
    value
        .parse::<u32>()
        .with_context(|| format!("runtime setting {} 不是有效整数", key))
}

fn parse_u64(key: &str, value: &str) -> anyhow::Result<u64> {
    value
        .parse::<u64>()
        .with_context(|| format!("runtime setting {} 不是有效整数", key))
}

fn parse_f64(key: &str, value: &str) -> anyhow::Result<f64> {
    value
        .parse::<f64>()
        .with_context(|| format!("runtime setting {} 不是有效数字", key))
}

fn parse_bool(key: &str, value: &str) -> anyhow::Result<bool> {
    value
        .parse::<bool>()
        .with_context(|| format!("runtime setting {} 不是有效布尔值", key))
}

fn stored_credential_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCredential> {
    let data_json: String = row.get(0)?;
    let mut credentials: KiroCredentials = serde_json::from_str(&data_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    credentials.canonicalize_auth_method();

    Ok(StoredCredential {
        credentials,
        policy: CredentialPolicy {
            max_concurrent_override: opt_i64_to_usize(row.get(1)?),
            rpm_override: opt_i64_to_u32(row.get(2)?),
        },
        failure_count: i64_to_u32(row.get(3)?),
        refresh_failure_count: i64_to_u32(row.get(4)?),
        success_count: i64_to_u64(row.get(5)?),
        last_used_at: row.get(6)?,
        disabled_reason: row.get(7)?,
    })
}

fn insert_or_replace_stored_credential_tx(
    conn: &rusqlite::Transaction<'_>,
    entry: &StoredCredential,
) -> anyhow::Result<()> {
    entry.policy.validate()?;
    let id = entry
        .credentials
        .id
        .ok_or_else(|| anyhow::anyhow!("持久化凭据时缺少 id"))?;
    let data_json = serde_json::to_string(&entry.credentials)?;
    conn.execute(
        r#"
        INSERT INTO credentials (
            id, data_json, max_concurrent_override, rpm_override, failure_count,
            refresh_failure_count, success_count, last_used_at, disabled_reason, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP)
        ON CONFLICT(id) DO UPDATE SET
            data_json = excluded.data_json,
            max_concurrent_override = excluded.max_concurrent_override,
            rpm_override = excluded.rpm_override,
            failure_count = excluded.failure_count,
            refresh_failure_count = excluded.refresh_failure_count,
            success_count = excluded.success_count,
            last_used_at = excluded.last_used_at,
            disabled_reason = excluded.disabled_reason,
            updated_at = CURRENT_TIMESTAMP
        "#,
        params![
            id,
            data_json,
            entry.policy.max_concurrent_override.map(|v| v as i64),
            entry.policy.rpm_override.map(|v| v as i64),
            entry.failure_count as i64,
            entry.refresh_failure_count as i64,
            entry.success_count as i64,
            entry.last_used_at,
            entry.disabled_reason
        ],
    )?;
    Ok(())
}

fn opt_i64_to_usize(value: Option<i64>) -> Option<usize> {
    value.and_then(|v| usize::try_from(v).ok())
}

fn opt_i64_to_u32(value: Option<i64>) -> Option<u32> {
    value.and_then(|v| u32::try_from(v).ok())
}

fn i64_to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kiro-rs-{}-{}.db", name, uuid::Uuid::new_v4()))
    }

    #[test]
    fn runtime_settings_round_trip() {
        let path = test_db_path("runtime-settings");
        let store = KiroStore::open(&path).unwrap();
        let mut defaults = RuntimeSettings::from_config(&Config::default());
        defaults.global_max_concurrent = 7;

        store.initialize_runtime_settings(&defaults).unwrap();
        let mut updated = defaults.clone();
        updated.global_max_concurrent = 11;
        updated.per_account_default_max_concurrent = 4;
        updated.load_balancing_mode = "balanced".to_string();
        store.save_runtime_settings(&updated).unwrap();

        let loaded = store.load_runtime_settings(&defaults).unwrap();
        assert_eq!(loaded.global_max_concurrent, 11);
        assert_eq!(loaded.per_account_default_max_concurrent, 4);
        assert_eq!(loaded.load_balancing_mode, "balanced");

        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn import_credentials_if_empty_does_not_duplicate_existing_db() {
        let path = test_db_path("import-once");
        let store = KiroStore::open(&path).unwrap();
        let config = Config::default();

        let mut first = KiroCredentials::default();
        first.id = Some(1);
        first.refresh_token = Some("first-refresh".to_string());
        let mut second = KiroCredentials::default();
        second.id = Some(2);
        second.refresh_token = Some("second-refresh".to_string());

        assert!(
            store
                .import_credentials_if_empty(&[first.clone()], &config)
                .unwrap()
        );
        assert!(
            !store
                .import_credentials_if_empty(&[second], &config)
                .unwrap()
        );

        let loaded = store.load_credentials().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].credentials.refresh_token.as_deref(),
            Some("first-refresh")
        );

        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
