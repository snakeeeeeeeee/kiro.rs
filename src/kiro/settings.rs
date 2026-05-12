use serde::{Deserialize, Serialize};

use crate::model::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub max_retry_accounts: usize,
    pub model_capacity_cooldown_ms: u64,
    pub token_auto_refresh_enabled: bool,
    pub token_auto_refresh_interval_secs: u64,
    pub token_auto_refresh_window_secs: u64,
    pub session_affinity_ttl_secs: u64,
    pub load_balancing_mode: String,
    pub virtual_cache_usage_enabled: bool,
    pub virtual_cache_default_ttl: String,
    pub virtual_cache_uncached_input_tokens: u32,
    pub virtual_cache_input_mode: String,
    pub virtual_cache_min_input_tokens: u32,
    pub virtual_cache_max_input_tokens: u32,
    pub virtual_cache_warmup_tokens: u32,
    pub virtual_cache_min_creation_tokens: u32,
    pub virtual_cache_max_creation_tokens: u32,
    pub virtual_cache_creation_mode: String,
    pub virtual_cache_creation_jitter_ratio: f64,
    pub virtual_cache_burst_every_turns: u32,
    pub virtual_cache_burst_min_tokens: u32,
    pub virtual_cache_burst_max_tokens: u32,
    pub virtual_cache_fallback_scope: String,
    pub dynamic_proxy_enabled: bool,
    pub dynamic_proxy_provider: String,
    pub dynamic_proxy_protocol: String,
    pub dynamic_proxy_host: String,
    pub dynamic_proxy_port: u16,
    pub dynamic_proxy_username_template: String,
    pub dynamic_proxy_password: String,
    pub dynamic_proxy_region: String,
    pub dynamic_proxy_state: String,
    pub dynamic_proxy_ttl_minutes: u32,
    pub dynamic_proxy_renew_before_ms: u64,
    pub dynamic_proxy_verify_url: String,
    pub dynamic_proxy_max_bind_retries: u32,
    pub dynamic_proxy_auto_bind_new_accounts: bool,
    pub dynamic_proxy_worker_interval_ms: u64,
    pub dynamic_proxy_worker_batch_size: usize,
    pub dynamic_proxy_worker_concurrency: usize,
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
            max_retry_accounts: config.max_retry_accounts.max(1),
            model_capacity_cooldown_ms: config.model_capacity_cooldown_ms,
            token_auto_refresh_enabled: config.token_auto_refresh_enabled,
            token_auto_refresh_interval_secs: config.token_auto_refresh_interval_secs,
            token_auto_refresh_window_secs: config.token_auto_refresh_window_secs,
            session_affinity_ttl_secs: config.session_affinity_ttl_secs,
            load_balancing_mode: normalize_load_balancing_mode(&config.load_balancing_mode),
            virtual_cache_usage_enabled: config.virtual_cache_usage_enabled,
            virtual_cache_default_ttl: normalize_virtual_cache_ttl(
                &config.virtual_cache_default_ttl,
            ),
            virtual_cache_uncached_input_tokens: config.virtual_cache_uncached_input_tokens.max(1),
            virtual_cache_input_mode: normalize_virtual_cache_input_mode(
                &config.virtual_cache_input_mode,
            ),
            virtual_cache_min_input_tokens: config.virtual_cache_min_input_tokens,
            virtual_cache_max_input_tokens: config.virtual_cache_max_input_tokens,
            virtual_cache_warmup_tokens: config.virtual_cache_warmup_tokens,
            virtual_cache_min_creation_tokens: config.virtual_cache_min_creation_tokens,
            virtual_cache_max_creation_tokens: config.virtual_cache_max_creation_tokens,
            virtual_cache_creation_mode: normalize_virtual_cache_creation_mode(
                &config.virtual_cache_creation_mode,
            ),
            virtual_cache_creation_jitter_ratio: config.virtual_cache_creation_jitter_ratio,
            virtual_cache_burst_every_turns: config.virtual_cache_burst_every_turns,
            virtual_cache_burst_min_tokens: config.virtual_cache_burst_min_tokens,
            virtual_cache_burst_max_tokens: config.virtual_cache_burst_max_tokens,
            virtual_cache_fallback_scope: normalize_virtual_cache_fallback_scope(
                &config.virtual_cache_fallback_scope,
            ),
            dynamic_proxy_enabled: config.dynamic_proxy_enabled,
            dynamic_proxy_provider: normalize_dynamic_proxy_provider(
                &config.dynamic_proxy_provider,
            ),
            dynamic_proxy_protocol: normalize_dynamic_proxy_protocol(
                &config.dynamic_proxy_protocol,
            ),
            dynamic_proxy_host: config.dynamic_proxy_host.trim().to_string(),
            dynamic_proxy_port: config.dynamic_proxy_port,
            dynamic_proxy_username_template: config.dynamic_proxy_username_template.clone(),
            dynamic_proxy_password: config.dynamic_proxy_password.clone(),
            dynamic_proxy_region: config.dynamic_proxy_region.clone(),
            dynamic_proxy_state: config.dynamic_proxy_state.clone(),
            dynamic_proxy_ttl_minutes: config.dynamic_proxy_ttl_minutes,
            dynamic_proxy_renew_before_ms: config.dynamic_proxy_renew_before_ms,
            dynamic_proxy_verify_url: config.dynamic_proxy_verify_url.clone(),
            dynamic_proxy_max_bind_retries: config.dynamic_proxy_max_bind_retries,
            dynamic_proxy_auto_bind_new_accounts: config.dynamic_proxy_auto_bind_new_accounts,
            dynamic_proxy_worker_interval_ms: config.dynamic_proxy_worker_interval_ms,
            dynamic_proxy_worker_batch_size: config.dynamic_proxy_worker_batch_size,
            dynamic_proxy_worker_concurrency: config.dynamic_proxy_worker_concurrency,
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
        validate_max_concurrent("maxRetryAccounts", self.max_retry_accounts, 128)?;
        validate_cooldown("modelCapacityCooldownMs", self.model_capacity_cooldown_ms)?;
        if !(30..=86_400).contains(&self.token_auto_refresh_interval_secs) {
            anyhow::bail!("tokenAutoRefreshIntervalSecs 必须在 30..86400 范围内");
        }
        if !(60..=86_400).contains(&self.token_auto_refresh_window_secs) {
            anyhow::bail!("tokenAutoRefreshWindowSecs 必须在 60..86400 范围内");
        }
        if !(300..=43_200).contains(&self.session_affinity_ttl_secs) {
            anyhow::bail!("sessionAffinityTtlSecs 必须在 300..43200 范围内");
        }
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
        if self.virtual_cache_input_mode != "fixed"
            && self.virtual_cache_input_mode != "estimated_user_delta"
        {
            anyhow::bail!("virtualCacheInputMode 必须是 'fixed' 或 'estimated_user_delta'");
        }
        validate_virtual_cache_tokens(
            "virtualCacheMinInputTokens",
            self.virtual_cache_min_input_tokens,
        )?;
        validate_virtual_cache_tokens(
            "virtualCacheMaxInputTokens",
            self.virtual_cache_max_input_tokens,
        )?;
        if self.virtual_cache_min_input_tokens == 0 {
            anyhow::bail!("virtualCacheMinInputTokens 必须大于 0");
        }
        if self.virtual_cache_min_input_tokens > self.virtual_cache_max_input_tokens {
            anyhow::bail!("virtualCacheMinInputTokens 不能大于 virtualCacheMaxInputTokens");
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
        if self.virtual_cache_creation_mode != "fixed"
            && self.virtual_cache_creation_mode != "dynamic"
        {
            anyhow::bail!("virtualCacheCreationMode 必须是 'fixed' 或 'dynamic'");
        }
        if !(0.0..=1.0).contains(&self.virtual_cache_creation_jitter_ratio) {
            anyhow::bail!("virtualCacheCreationJitterRatio 必须在 0..1 范围内");
        }
        validate_virtual_cache_tokens(
            "virtualCacheBurstMinTokens",
            self.virtual_cache_burst_min_tokens,
        )?;
        validate_virtual_cache_tokens(
            "virtualCacheBurstMaxTokens",
            self.virtual_cache_burst_max_tokens,
        )?;
        if self.virtual_cache_burst_min_tokens > self.virtual_cache_burst_max_tokens {
            anyhow::bail!("virtualCacheBurstMinTokens 不能大于 virtualCacheBurstMaxTokens");
        }
        if self.virtual_cache_fallback_scope != "model"
            && self.virtual_cache_fallback_scope != "none"
        {
            anyhow::bail!("virtualCacheFallbackScope 必须是 'model' 或 'none'");
        }
        if self.dynamic_proxy_provider.trim().is_empty() {
            anyhow::bail!("dynamicProxyProvider 不能为空");
        }
        if self.dynamic_proxy_protocol != "http" && self.dynamic_proxy_protocol != "socks5" {
            anyhow::bail!("dynamicProxyProtocol 必须是 'http' 或 'socks5'");
        }
        if self.dynamic_proxy_enabled && self.dynamic_proxy_host.trim().is_empty() {
            anyhow::bail!("dynamicProxyHost 不能为空");
        }
        if self.dynamic_proxy_port == 0 {
            anyhow::bail!("dynamicProxyPort 必须在 1..65535 范围内");
        }
        if self.dynamic_proxy_enabled && self.dynamic_proxy_username_template.trim().is_empty() {
            anyhow::bail!("dynamicProxyUsernameTemplate 不能为空");
        }
        if !(1..=24 * 60).contains(&self.dynamic_proxy_ttl_minutes) {
            anyhow::bail!("dynamicProxyTtlMinutes 必须在 1..1440 范围内");
        }
        if self.dynamic_proxy_renew_before_ms > 86_400_000 {
            anyhow::bail!("dynamicProxyRenewBeforeMs 不能超过 86400000");
        }
        if self.dynamic_proxy_enabled && self.dynamic_proxy_verify_url.trim().is_empty() {
            anyhow::bail!("dynamicProxyVerifyUrl 不能为空");
        }
        if !(1..=20).contains(&self.dynamic_proxy_max_bind_retries) {
            anyhow::bail!("dynamicProxyMaxBindRetries 必须在 1..20 范围内");
        }
        if !(1_000..=86_400_000).contains(&self.dynamic_proxy_worker_interval_ms) {
            anyhow::bail!("dynamicProxyWorkerIntervalMs 必须在 1000..86400000 范围内");
        }
        if self.dynamic_proxy_worker_batch_size > 1_000 {
            anyhow::bail!("dynamicProxyWorkerBatchSize 必须在 0..1000 范围内");
        }
        if self.dynamic_proxy_worker_concurrency == 0 || self.dynamic_proxy_worker_concurrency > 100
        {
            anyhow::bail!("dynamicProxyWorkerConcurrency 必须在 1..100 范围内");
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

pub fn normalize_virtual_cache_input_mode(mode: &str) -> String {
    if mode == "estimated_user_delta" {
        "estimated_user_delta".to_string()
    } else {
        "fixed".to_string()
    }
}

pub fn normalize_virtual_cache_creation_mode(mode: &str) -> String {
    if mode == "dynamic" {
        "dynamic".to_string()
    } else {
        "fixed".to_string()
    }
}

pub fn normalize_virtual_cache_fallback_scope(scope: &str) -> String {
    if scope == "none" {
        "none".to_string()
    } else {
        "model".to_string()
    }
}

pub fn normalize_dynamic_proxy_provider(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        "novproxy".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn normalize_dynamic_proxy_protocol(protocol: &str) -> String {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "socks" | "socks5h" | "socks5" => "socks5".to_string(),
        _ => "http".to_string(),
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
