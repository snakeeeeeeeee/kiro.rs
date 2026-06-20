use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::Utc;
use parking_lot::RwLock;

use crate::common::auth;
use crate::kiro::store::{KiroStore, StoredApiKey};

const DEFAULT_API_KEY_NAME: &str = "未命名密钥";
const LAST_USED_FLUSH_INTERVAL: Duration = Duration::from_secs(60);
#[cfg(test)]
const API_KEY_HEX_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: u64,
    pub name: String,
    pub key: String,
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone)]
struct ApiKeyCacheEntry {
    record: ApiKeyRecord,
    last_used_flush_at: Option<Instant>,
}

#[derive(Clone)]
pub struct ApiKeyManager {
    store: KiroStore,
    cache: Arc<RwLock<Vec<ApiKeyCacheEntry>>>,
}

impl ApiKeyManager {
    pub fn new(store: KiroStore) -> anyhow::Result<Self> {
        let records = store
            .load_api_keys()
            .context("加载外部 API 密钥失败")?
            .into_iter()
            .filter(|record| !record.disabled)
            .map(|record| ApiKeyCacheEntry {
                record: record.into(),
                last_used_flush_at: None,
            })
            .collect();

        Ok(Self {
            store,
            cache: Arc::new(RwLock::new(records)),
        })
    }

    pub fn list(&self) -> anyhow::Result<Vec<ApiKeyRecord>> {
        let mut records: Vec<ApiKeyRecord> = self
            .store
            .load_api_keys()?
            .into_iter()
            .map(Into::into)
            .collect();
        records.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(records)
    }

    pub fn create(&self, name: Option<String>) -> anyhow::Result<ApiKeyRecord> {
        let name = normalize_api_key_name(name);
        for _ in 0..5 {
            let key = generate_api_key();
            match self.store.insert_api_key(&name, &key) {
                Ok(record) => {
                    let record: ApiKeyRecord = record.into();
                    self.upsert_cache(record.clone());
                    return Ok(record);
                }
                Err(err) if err.to_string().contains("UNIQUE") => continue,
                Err(err) => return Err(err),
            }
        }
        anyhow::bail!("生成唯一密钥失败，请重试")
    }

    pub fn update(
        &self,
        id: u64,
        name: Option<String>,
        disabled: Option<bool>,
    ) -> anyhow::Result<ApiKeyRecord> {
        let name = name.map(|value| normalize_api_key_name(Some(value)));
        let record = self
            .store
            .update_api_key(id, name.as_deref(), disabled)
            .map(ApiKeyRecord::from)?;
        self.sync_cache_record(record.clone());
        Ok(record)
    }

    pub fn delete(&self, id: u64) -> anyhow::Result<bool> {
        let deleted = self.store.delete_api_key(id)?;
        if deleted {
            self.remove_from_cache(id);
        }
        Ok(deleted)
    }

    pub fn authenticate(&self, key: &str) -> bool {
        let mut matched_id = None;
        {
            let cache = self.cache.read();
            for entry in cache.iter() {
                if auth::constant_time_eq(key, &entry.record.key) {
                    matched_id = Some(entry.record.id);
                    break;
                }
            }
        }

        if let Some(id) = matched_id {
            self.touch_last_used_throttled(id);
            true
        } else {
            false
        }
    }

    fn sync_cache_record(&self, record: ApiKeyRecord) {
        if record.disabled {
            self.remove_from_cache(record.id);
        } else {
            self.upsert_cache(record);
        }
    }

    fn upsert_cache(&self, record: ApiKeyRecord) {
        let mut cache = self.cache.write();
        if let Some(entry) = cache.iter_mut().find(|entry| entry.record.id == record.id) {
            entry.record = record;
            return;
        }
        cache.push(ApiKeyCacheEntry {
            record,
            last_used_flush_at: None,
        });
    }

    fn remove_from_cache(&self, id: u64) {
        let mut cache = self.cache.write();
        cache.retain(|entry| entry.record.id != id);
    }

    fn touch_last_used_throttled(&self, id: u64) {
        let now = Instant::now();
        let used_at = Utc::now().to_rfc3339();
        let should_flush = {
            let mut cache = self.cache.write();
            let Some(entry) = cache.iter_mut().find(|entry| entry.record.id == id) else {
                return;
            };
            entry.record.last_used_at = Some(used_at.clone());
            let should_flush = entry
                .last_used_flush_at
                .is_none_or(|last| now.duration_since(last) >= LAST_USED_FLUSH_INTERVAL);
            if should_flush {
                entry.last_used_flush_at = Some(now);
            }
            should_flush
        };

        if should_flush {
            if let Err(err) = self.store.touch_api_key_last_used(id, &used_at) {
                tracing::warn!(api_key_id = id, error = %err, "更新外部 API 密钥最近使用时间失败");
            }
        }
    }
}

impl From<StoredApiKey> for ApiKeyRecord {
    fn from(value: StoredApiKey) -> Self {
        Self {
            id: value.id,
            name: value.name,
            key: value.key,
            disabled: value.disabled,
            created_at: value.created_at,
            updated_at: value.updated_at,
            last_used_at: value.last_used_at,
        }
    }
}

pub fn normalize_api_key_name(name: Option<String>) -> String {
    let name = name.unwrap_or_default().trim().to_string();
    if name.is_empty() {
        DEFAULT_API_KEY_NAME.to_string()
    } else {
        name
    }
}

pub fn generate_api_key() -> String {
    let random = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    format!("sk-{}", random)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> KiroStore {
        let path = std::env::temp_dir().join(format!(
            "kiro-api-key-manager-{name}-{}.db",
            uuid::Uuid::new_v4()
        ));
        KiroStore::open(path).unwrap()
    }

    #[test]
    fn generated_key_has_expected_prefix() {
        let key = generate_api_key();
        assert!(key.starts_with("sk-"));
        assert_eq!(key.len(), API_KEY_HEX_LEN + 3);
    }

    #[test]
    fn create_disable_delete_keeps_cache_in_sync() {
        let manager = ApiKeyManager::new(test_store("cache-sync")).unwrap();
        let record = manager.create(Some("test key".to_string())).unwrap();
        assert!(manager.authenticate(&record.key));

        manager.update(record.id, None, Some(true)).unwrap();
        assert!(!manager.authenticate(&record.key));

        manager.update(record.id, None, Some(false)).unwrap();
        assert!(manager.authenticate(&record.key));

        assert!(manager.delete(record.id).unwrap());
        assert!(!manager.authenticate(&record.key));
    }

    #[test]
    fn empty_name_uses_default() {
        assert_eq!(
            normalize_api_key_name(Some("   ".to_string())),
            DEFAULT_API_KEY_NAME
        );
    }
}
