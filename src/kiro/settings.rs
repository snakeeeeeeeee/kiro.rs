use serde::{Deserialize, Serialize};

use crate::model::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    pub global_max_concurrent: usize,
    pub per_account_default_max_concurrent: usize,
    pub queue_max_size: usize,
    pub queue_timeout_ms: u64,
    pub per_account_default_rpm: u32,
    pub global_rpm: u32,
    pub rate_limit_cooldown_ms: u64,
    pub transient_cooldown_ms: u64,
    pub load_balancing_mode: String,
    pub virtual_cache_usage_enabled: bool,
    pub virtual_cache_default_ttl: String,
    pub virtual_cache_uncached_input_tokens: u32,
    pub virtual_cache_warmup_tokens: u32,
    pub virtual_cache_min_creation_tokens: u32,
    pub virtual_cache_max_creation_tokens: u32,
    pub virtual_cache_fallback_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPolicy {
    pub max_concurrent_override: Option<usize>,
    pub rpm_override: Option<u32>,
}

impl RuntimeSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            global_max_concurrent: config.global_max_concurrent.max(1),
            per_account_default_max_concurrent: config.per_account_max_concurrent.max(1),
            queue_max_size: config.queue_max_size,
            queue_timeout_ms: config.queue_timeout_ms.max(1_000),
            per_account_default_rpm: config.per_account_rpm,
            global_rpm: config.global_rpm,
            rate_limit_cooldown_ms: config.rate_limit_cooldown_ms,
            transient_cooldown_ms: config.transient_cooldown_ms,
            load_balancing_mode: normalize_load_balancing_mode(&config.load_balancing_mode),
            virtual_cache_usage_enabled: config.virtual_cache_usage_enabled,
            virtual_cache_default_ttl: normalize_virtual_cache_ttl(
                &config.virtual_cache_default_ttl,
            ),
            virtual_cache_uncached_input_tokens: config.virtual_cache_uncached_input_tokens.max(1),
            virtual_cache_warmup_tokens: config.virtual_cache_warmup_tokens,
            virtual_cache_min_creation_tokens: config.virtual_cache_min_creation_tokens,
            virtual_cache_max_creation_tokens: config.virtual_cache_max_creation_tokens,
            virtual_cache_fallback_scope: normalize_virtual_cache_fallback_scope(
                &config.virtual_cache_fallback_scope,
            ),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_max_concurrent("globalMaxConcurrent", self.global_max_concurrent, 512)?;
        validate_max_concurrent(
            "perAccountDefaultMaxConcurrent",
            self.per_account_default_max_concurrent,
            64,
        )?;
        if self.queue_max_size > 10_000 {
            anyhow::bail!("queueMaxSize 必须在 0..10000 范围内");
        }
        if !(1_000..=300_000).contains(&self.queue_timeout_ms) {
            anyhow::bail!("queueTimeoutMs 必须在 1000..300000 范围内");
        }
        validate_rpm("perAccountDefaultRpm", self.per_account_default_rpm)?;
        validate_rpm("globalRpm", self.global_rpm)?;
        validate_cooldown("rateLimitCooldownMs", self.rate_limit_cooldown_ms)?;
        validate_cooldown("transientCooldownMs", self.transient_cooldown_ms)?;
        if self.load_balancing_mode != "priority" && self.load_balancing_mode != "balanced" {
            anyhow::bail!("loadBalancingMode 必须是 'priority' 或 'balanced'");
        }
        if self.virtual_cache_default_ttl != "5m" && self.virtual_cache_default_ttl != "1h" {
            anyhow::bail!("virtualCacheDefaultTtl 必须是 '5m' 或 '1h'");
        }
        if self.virtual_cache_uncached_input_tokens == 0
            || self.virtual_cache_uncached_input_tokens > 10_000
        {
            anyhow::bail!("virtualCacheUncachedInputTokens 必须在 1..10000 范围内");
        }
        validate_virtual_cache_tokens(
            "virtualCacheWarmupTokens",
            self.virtual_cache_warmup_tokens,
        )?;
        validate_virtual_cache_tokens(
            "virtualCacheMinCreationTokens",
            self.virtual_cache_min_creation_tokens,
        )?;
        validate_virtual_cache_tokens(
            "virtualCacheMaxCreationTokens",
            self.virtual_cache_max_creation_tokens,
        )?;
        if self.virtual_cache_min_creation_tokens > self.virtual_cache_max_creation_tokens {
            anyhow::bail!("virtualCacheMinCreationTokens 不能大于 virtualCacheMaxCreationTokens");
        }
        if self.virtual_cache_fallback_scope != "model"
            && self.virtual_cache_fallback_scope != "none"
        {
            anyhow::bail!("virtualCacheFallbackScope 必须是 'model' 或 'none'");
        }
        Ok(())
    }
}

impl CredentialPolicy {
    pub fn default() -> Self {
        Self {
            max_concurrent_override: None,
            rpm_override: None,
        }
    }

    pub fn effective_max_concurrent(&self, settings: &RuntimeSettings) -> usize {
        self.max_concurrent_override
            .unwrap_or(settings.per_account_default_max_concurrent)
            .max(1)
    }

    pub fn effective_rpm(&self, settings: &RuntimeSettings) -> u32 {
        self.rpm_override
            .unwrap_or(settings.per_account_default_rpm)
    }

    pub fn uses_default_policy(&self) -> bool {
        self.max_concurrent_override.is_none() && self.rpm_override.is_none()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(value) = self.max_concurrent_override {
            validate_max_concurrent("maxConcurrentOverride", value, 64)?;
        }
        if let Some(value) = self.rpm_override {
            validate_rpm("rpmOverride", value)?;
        }
        Ok(())
    }
}

pub fn normalize_load_balancing_mode(mode: &str) -> String {
    if mode == "balanced" {
        "balanced".to_string()
    } else {
        "priority".to_string()
    }
}

pub fn normalize_virtual_cache_ttl(ttl: &str) -> String {
    if ttl == "1h" {
        "1h".to_string()
    } else {
        "5m".to_string()
    }
}

pub fn normalize_virtual_cache_fallback_scope(scope: &str) -> String {
    if scope == "none" {
        "none".to_string()
    } else {
        "model".to_string()
    }
}

fn validate_max_concurrent(name: &str, value: usize, max: usize) -> anyhow::Result<()> {
    if value == 0 || value > max {
        anyhow::bail!("{} 必须在 1..{} 范围内", name, max);
    }
    Ok(())
}

fn validate_rpm(name: &str, value: u32) -> anyhow::Result<()> {
    if value > 10_000 {
        anyhow::bail!("{} 必须在 0..10000 范围内，0 表示不限速", name);
    }
    Ok(())
}

fn validate_cooldown(name: &str, value: u64) -> anyhow::Result<()> {
    if value > 3_600_000 {
        anyhow::bail!("{} 不能超过 3600000ms", name);
    }
    Ok(())
}

fn validate_virtual_cache_tokens(name: &str, value: u32) -> anyhow::Result<()> {
    if value > 10_000_000 {
        anyhow::bail!("{} 不能超过 10000000", name);
    }
    Ok(())
}
