use serde::{Deserialize, Serialize};

use crate::kiro::endpoint::{is_supported_endpoint_name, normalize_endpoint_name};
use crate::model::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SameAccountRetryRule {
    #[serde(default = "default_same_account_retry_rule_enabled")]
    pub enabled: bool,
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub attempts: usize,
    pub delay_ms: u64,
    #[serde(default = "default_same_account_retry_rule_respect_retry_after")]
    pub respect_retry_after: bool,
}

fn default_same_account_retry_rule_enabled() -> bool {
    true
}

fn default_same_account_retry_rule_respect_retry_after() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    pub default_endpoint: String,
    pub global_max_concurrent: usize,
    pub global_max_concurrent_limit: usize,
    pub per_account_default_max_concurrent: usize,
    pub queue_max_size: usize,
    pub queue_timeout_ms: u64,
    pub per_account_default_rpm: u32,
    pub global_rpm: u32,
    pub rate_limit_cooldown_ms: u64,
    pub transient_cooldown_ms: u64,
    pub max_retry_accounts: usize,
    pub allow_over_usage: bool,
    pub model_capacity_cooldown_ms: u64,
    pub same_account_retry_rules: Vec<SameAccountRetryRule>,
    pub token_auto_refresh_enabled: bool,
    pub token_auto_refresh_interval_secs: u64,
    pub token_auto_refresh_window_secs: u64,
    pub session_affinity_enabled: bool,
    pub session_affinity_ttl_secs: u64,
    pub opus47_plain_stabilization_mode: String,
    pub opus47_antml_probe_compat: String,
    pub opus47_clean_probe_mode: String,
    pub opus47_detection_profile: String,
    pub opus47_signed_thinking_preservation: String,
    pub opus47_short_thinking_experiment: String,
    pub opus47_diagnostics_enabled: bool,
    pub opus47_raw_debug_enabled: bool,
    pub opus47_raw_debug_max_chars: usize,
    pub opus46_detection_profile: String,
    pub opus46_antml_probe_compat: String,
    pub opus46_diagnostics_enabled: bool,
    pub opus46_raw_debug_enabled: bool,
    pub opus46_raw_debug_max_chars: usize,
    pub sonnet46_detection_profile: String,
    pub sonnet46_antml_probe_compat: String,
    pub sonnet46_diagnostics_enabled: bool,
    pub sonnet46_raw_debug_enabled: bool,
    pub sonnet46_raw_debug_max_chars: usize,
    pub prompt_dump_enabled: bool,
    pub prompt_dump_dir: String,
    pub prompt_dump_max_bytes: usize,
    pub prompt_dump_models: String,
    pub message_pruning_enabled: bool,
    pub message_pruning_max_request_bytes: usize,
    pub message_pruning_keep_recent_messages: usize,
    pub message_pruning_max_history_entry_bytes: usize,
    pub message_pruning_max_truncated_content_bytes: usize,
    pub compat_usage_shape: String,
    pub compat_thinking_model: String,
    pub compat_models_shape: String,
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
    #[serde(default)]
    pub virtual_cache_haiku_input_only_enabled: bool,
    pub target_cache_reuse_ratio: f64,
    pub virtual_cache_context_shrink_reset_ratio: f64,
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
    pub allow_overage: bool,
    pub overage_weight: u32,
    #[serde(default = "default_turbo_mode")]
    pub turbo_mode: String,
    #[serde(default = "default_turbo_fanout")]
    pub turbo_fanout: usize,
}

fn default_turbo_mode() -> String {
    "off".to_string()
}

fn default_turbo_fanout() -> usize {
    1
}

impl RuntimeSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            default_endpoint: normalize_endpoint_name(&config.default_endpoint),
            global_max_concurrent: config.global_max_concurrent.max(1),
            global_max_concurrent_limit: config.global_max_concurrent_limit.max(1),
            per_account_default_max_concurrent: config.per_account_max_concurrent.max(1),
            queue_max_size: config.queue_max_size,
            queue_timeout_ms: config.queue_timeout_ms.max(1_000),
            per_account_default_rpm: config.per_account_rpm,
            global_rpm: config.global_rpm,
            rate_limit_cooldown_ms: config.rate_limit_cooldown_ms,
            transient_cooldown_ms: config.transient_cooldown_ms,
            max_retry_accounts: config.max_retry_accounts.max(1),
            allow_over_usage: config.allow_over_usage,
            model_capacity_cooldown_ms: config.model_capacity_cooldown_ms,
            same_account_retry_rules: config.same_account_retry_rules.clone(),
            token_auto_refresh_enabled: config.token_auto_refresh_enabled,
            token_auto_refresh_interval_secs: config.token_auto_refresh_interval_secs,
            token_auto_refresh_window_secs: config.token_auto_refresh_window_secs,
            session_affinity_enabled: config.session_affinity_enabled,
            session_affinity_ttl_secs: config.session_affinity_ttl_secs,
            opus47_plain_stabilization_mode: normalize_opus47_plain_stabilization_mode(
                &config.opus47_plain_stabilization_mode,
            ),
            opus47_antml_probe_compat: normalize_opus47_antml_probe_compat(
                &config.opus47_antml_probe_compat,
            ),
            opus47_clean_probe_mode: normalize_opus47_clean_probe_mode(
                &config.opus47_clean_probe_mode,
            ),
            opus47_detection_profile: normalize_opus47_detection_profile(
                &config.opus47_detection_profile,
            ),
            opus47_signed_thinking_preservation: normalize_opus47_signed_thinking_preservation(
                &config.opus47_signed_thinking_preservation,
            ),
            opus47_short_thinking_experiment: normalize_opus47_short_thinking_experiment(
                &config.opus47_short_thinking_experiment,
            ),
            opus47_diagnostics_enabled: config.opus47_diagnostics_enabled,
            opus47_raw_debug_enabled: config.opus47_raw_debug_enabled,
            opus47_raw_debug_max_chars: config.opus47_raw_debug_max_chars.clamp(1_000, 200_000),
            opus46_detection_profile: normalize_model46_detection_profile(
                &config.opus46_detection_profile,
            ),
            opus46_antml_probe_compat: normalize_opus47_antml_probe_compat(
                &config.opus46_antml_probe_compat,
            ),
            opus46_diagnostics_enabled: config.opus46_diagnostics_enabled,
            opus46_raw_debug_enabled: config.opus46_raw_debug_enabled,
            opus46_raw_debug_max_chars: config.opus46_raw_debug_max_chars.clamp(1_000, 200_000),
            sonnet46_detection_profile: normalize_model46_detection_profile(
                &config.sonnet46_detection_profile,
            ),
            sonnet46_antml_probe_compat: normalize_opus47_antml_probe_compat(
                &config.sonnet46_antml_probe_compat,
            ),
            sonnet46_diagnostics_enabled: config.sonnet46_diagnostics_enabled,
            sonnet46_raw_debug_enabled: config.sonnet46_raw_debug_enabled,
            sonnet46_raw_debug_max_chars: config.sonnet46_raw_debug_max_chars.clamp(1_000, 200_000),
            prompt_dump_enabled: config.prompt_dump_enabled,
            prompt_dump_dir: normalize_prompt_dump_dir(&config.prompt_dump_dir),
            prompt_dump_max_bytes: config.prompt_dump_max_bytes.clamp(10_000, 50_000_000),
            prompt_dump_models: normalize_prompt_dump_models(&config.prompt_dump_models),
            message_pruning_enabled: config.message_pruning_enabled,
            message_pruning_max_request_bytes: config.message_pruning_max_request_bytes,
            message_pruning_keep_recent_messages: config.message_pruning_keep_recent_messages,
            message_pruning_max_history_entry_bytes: config.message_pruning_max_history_entry_bytes,
            message_pruning_max_truncated_content_bytes: config
                .message_pruning_max_truncated_content_bytes,
            compat_usage_shape: normalize_compat_usage_shape(&config.compat_usage_shape),
            compat_thinking_model: normalize_compat_thinking_model(&config.compat_thinking_model),
            compat_models_shape: normalize_compat_models_shape(&config.compat_models_shape),
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
            virtual_cache_haiku_input_only_enabled: config.virtual_cache_haiku_input_only_enabled,
            target_cache_reuse_ratio: config.target_cache_reuse_ratio.clamp(0.0, 1.0),
            virtual_cache_context_shrink_reset_ratio: config
                .virtual_cache_context_shrink_reset_ratio
                .clamp(0.0, 1.0),
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
        if self.default_endpoint.trim().is_empty() {
            anyhow::bail!("defaultEndpoint 不能为空");
        }
        if !is_supported_endpoint_name(&self.default_endpoint) {
            anyhow::bail!("defaultEndpoint 未知: {}", self.default_endpoint);
        }
        validate_max_concurrent(
            "globalMaxConcurrentLimit",
            self.global_max_concurrent_limit,
            65_536,
        )?;
        validate_max_concurrent(
            "globalMaxConcurrent",
            self.global_max_concurrent,
            self.global_max_concurrent_limit,
        )?;
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
        validate_same_account_retry_rules(&self.same_account_retry_rules)?;
        if !(30..=86_400).contains(&self.token_auto_refresh_interval_secs) {
            anyhow::bail!("tokenAutoRefreshIntervalSecs 必须在 30..86400 范围内");
        }
        if !(60..=86_400).contains(&self.token_auto_refresh_window_secs) {
            anyhow::bail!("tokenAutoRefreshWindowSecs 必须在 60..86400 范围内");
        }
        if !(300..=43_200).contains(&self.session_affinity_ttl_secs) {
            anyhow::bail!("sessionAffinityTtlSecs 必须在 300..43200 范围内");
        }
        if !matches!(
            self.opus47_plain_stabilization_mode.as_str(),
            "off" | "adaptive_low" | "adaptive_high"
        ) {
            anyhow::bail!(
                "opus47PlainStabilizationMode 必须是 'off'、'adaptive_low' 或 'adaptive_high'"
            );
        }
        if !matches!(self.opus47_antml_probe_compat.as_str(), "off" | "clarify") {
            anyhow::bail!("opus47AntmlProbeCompat 必须是 'off' 或 'clarify'");
        }
        if !matches!(self.opus47_clean_probe_mode.as_str(), "off" | "clean") {
            anyhow::bail!("opus47CleanProbeMode 必须是 'off' 或 'clean'");
        }
        if !matches!(
            self.opus47_detection_profile.as_str(),
            "normal" | "cc_max_like" | "clean_probe_debug"
        ) {
            anyhow::bail!(
                "opus47DetectionProfile 必须是 'normal'、'cc_max_like' 或 'clean_probe_debug'"
            );
        }
        if !matches!(
            self.opus47_signed_thinking_preservation.as_str(),
            "off" | "diagnose" | "cache_only" | "history_experiment"
        ) {
            anyhow::bail!(
                "opus47SignedThinkingPreservation 必须是 'off'、'diagnose'、'cache_only' 或 'history_experiment'"
            );
        }
        if !matches!(
            self.opus47_short_thinking_experiment.as_str(),
            "off" | "adaptive_high"
        ) {
            anyhow::bail!("opus47ShortThinkingExperiment 必须是 'off' 或 'adaptive_high'");
        }
        if self.compat_usage_shape != "anthropic" && self.compat_usage_shape != "flat" {
            anyhow::bail!("compatUsageShape 必须是 'anthropic' 或 'flat'");
        }
        if !(1_000..=200_000).contains(&self.opus47_raw_debug_max_chars) {
            anyhow::bail!("opus47RawDebugMaxChars 必须在 1000..200000 范围内");
        }
        if !matches!(
            self.opus46_detection_profile.as_str(),
            "normal" | "cc_max_like"
        ) {
            anyhow::bail!("opus46DetectionProfile 必须是 'normal' 或 'cc_max_like'");
        }
        if !matches!(self.opus46_antml_probe_compat.as_str(), "off" | "clarify") {
            anyhow::bail!("opus46AntmlProbeCompat 必须是 'off' 或 'clarify'");
        }
        if !(1_000..=200_000).contains(&self.opus46_raw_debug_max_chars) {
            anyhow::bail!("opus46RawDebugMaxChars 必须在 1000..200000 范围内");
        }
        if !matches!(
            self.sonnet46_detection_profile.as_str(),
            "normal" | "cc_max_like"
        ) {
            anyhow::bail!("sonnet46DetectionProfile 必须是 'normal' 或 'cc_max_like'");
        }
        if !matches!(self.sonnet46_antml_probe_compat.as_str(), "off" | "clarify") {
            anyhow::bail!("sonnet46AntmlProbeCompat 必须是 'off' 或 'clarify'");
        }
        if !(1_000..=200_000).contains(&self.sonnet46_raw_debug_max_chars) {
            anyhow::bail!("sonnet46RawDebugMaxChars 必须在 1000..200000 范围内");
        }
        if self.prompt_dump_dir.trim().is_empty() {
            anyhow::bail!("promptDumpDir 不能为空");
        }
        if !(10_000..=50_000_000).contains(&self.prompt_dump_max_bytes) {
            anyhow::bail!("promptDumpMaxBytes 必须在 10000..50000000 范围内");
        }
        if !(100_000..=5_000_000).contains(&self.message_pruning_max_request_bytes) {
            anyhow::bail!("messagePruningMaxRequestBytes 必须在 100000..5000000 范围内");
        }
        if !(1..=200).contains(&self.message_pruning_keep_recent_messages) {
            anyhow::bail!("messagePruningKeepRecentMessages 必须在 1..200 范围内");
        }
        if !(10_000..=2_000_000).contains(&self.message_pruning_max_history_entry_bytes) {
            anyhow::bail!("messagePruningMaxHistoryEntryBytes 必须在 10000..2000000 范围内");
        }
        if !(1_000..=500_000).contains(&self.message_pruning_max_truncated_content_bytes) {
            anyhow::bail!("messagePruningMaxTruncatedContentBytes 必须在 1000..500000 范围内");
        }
        if self.message_pruning_max_truncated_content_bytes
            > self.message_pruning_max_history_entry_bytes
        {
            anyhow::bail!(
                "messagePruningMaxTruncatedContentBytes 不能大于 messagePruningMaxHistoryEntryBytes"
            );
        }
        if self.compat_thinking_model != "native" && self.compat_thinking_model != "plain_text" {
            anyhow::bail!("compatThinkingModel 必须是 'native' 或 'plain_text'");
        }
        if self.compat_models_shape != "anthropic" && self.compat_models_shape != "aggregator" {
            anyhow::bail!("compatModelsShape 必须是 'anthropic' 或 'aggregator'");
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
        if !(0.0..=1.0).contains(&self.target_cache_reuse_ratio) {
            anyhow::bail!("targetCacheReuseRatio 必须在 0..1 范围内，0 表示关闭");
        }
        if !(0.0..=1.0).contains(&self.virtual_cache_context_shrink_reset_ratio) {
            anyhow::bail!("virtualCacheContextShrinkResetRatio 必须在 0..1 范围内，0 表示关闭");
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
            allow_overage: false,
            overage_weight: 1,
            turbo_mode: default_turbo_mode(),
            turbo_fanout: default_turbo_fanout(),
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
        self.max_concurrent_override.is_none()
            && self.rpm_override.is_none()
            && !self.allow_overage
            && self.effective_overage_weight() == 1
            && self.effective_turbo_mode() == "off"
            && self.effective_turbo_fanout() == 1
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(value) = self.max_concurrent_override {
            validate_max_concurrent("maxConcurrentOverride", value, 64)?;
        }
        if let Some(value) = self.rpm_override {
            validate_rpm("rpmOverride", value)?;
        }
        if !(1..=10).contains(&self.overage_weight) {
            anyhow::bail!("overageWeight 必须在 1..10 范围内");
        }
        let turbo_mode = self.effective_turbo_mode();
        if turbo_mode != "off" && turbo_mode != "race" {
            anyhow::bail!("turboMode 必须是 'off' 或 'race'");
        }
        if turbo_mode == "race" && !(2..=5).contains(&self.turbo_fanout) {
            anyhow::bail!("turboFanout 必须在 2..5 范围内");
        }
        if turbo_mode == "off" && self.turbo_fanout > 5 {
            anyhow::bail!("turboFanout 不能超过 5");
        }
        Ok(())
    }

    pub fn effective_overage_weight(&self) -> u32 {
        self.overage_weight.clamp(1, 10)
    }

    pub fn effective_turbo_mode(&self) -> &str {
        let mode = self.turbo_mode.trim();
        if mode.eq_ignore_ascii_case("race") {
            "race"
        } else if mode.eq_ignore_ascii_case("off") || mode.is_empty() {
            "off"
        } else {
            "__invalid__"
        }
    }

    pub fn effective_turbo_fanout(&self) -> usize {
        if self.effective_turbo_mode() == "race" {
            self.turbo_fanout.clamp(2, 5)
        } else {
            1
        }
    }
}

pub fn normalize_load_balancing_mode(mode: &str) -> String {
    if mode == "balanced" {
        "balanced".to_string()
    } else {
        "priority".to_string()
    }
}

pub fn normalize_opus47_plain_stabilization_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "adaptive_low" => "adaptive_low".to_string(),
        "adaptive_high" => "adaptive_high".to_string(),
        _ => "off".to_string(),
    }
}

pub fn normalize_opus47_antml_probe_compat(mode: &str) -> String {
    if mode.trim().eq_ignore_ascii_case("clarify") {
        "clarify".to_string()
    } else {
        "off".to_string()
    }
}

pub fn normalize_opus47_clean_probe_mode(mode: &str) -> String {
    if mode.trim().eq_ignore_ascii_case("clean") {
        "clean".to_string()
    } else {
        "off".to_string()
    }
}

pub fn normalize_opus47_detection_profile(profile: &str) -> String {
    match profile.trim().to_ascii_lowercase().as_str() {
        "cc_max_like" | "cc-max-like" | "ccmaxlike" => "cc_max_like".to_string(),
        "clean_probe_debug" | "clean-probe-debug" => "clean_probe_debug".to_string(),
        _ => "normal".to_string(),
    }
}

pub fn normalize_model46_detection_profile(profile: &str) -> String {
    match profile.trim().to_ascii_lowercase().as_str() {
        "cc_max_like" | "cc-max-like" | "ccmaxlike" => "cc_max_like".to_string(),
        _ => "normal".to_string(),
    }
}

pub fn normalize_opus47_signed_thinking_preservation(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "diagnose" => "diagnose".to_string(),
        "cache_only" | "cache-only" => "cache_only".to_string(),
        "history_experiment" | "history-experiment" => "history_experiment".to_string(),
        _ => "off".to_string(),
    }
}

pub fn normalize_opus47_short_thinking_experiment(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "adaptive_high" | "adaptive-high" => "adaptive_high".to_string(),
        _ => "off".to_string(),
    }
}

pub fn normalize_prompt_dump_dir(dir: &str) -> String {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        "/app/config/prompt-dumps".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn normalize_prompt_dump_models(models: &str) -> String {
    let normalized: Vec<String> = models
        .split(',')
        .map(|model| model.trim().to_ascii_lowercase())
        .filter(|model| !model.is_empty())
        .collect();
    if normalized.is_empty() {
        "claude-opus-4-6,claude-opus-4-7,claude-opus-4-8,claude-sonnet-4-6,claude-sonnet-5"
            .to_string()
    } else {
        normalized.join(",")
    }
}

pub fn effective_opus47_clean_probe_mode(settings: &RuntimeSettings) -> String {
    match settings.opus47_detection_profile.as_str() {
        "clean_probe_debug" => "clean".to_string(),
        "cc_max_like" => "off".to_string(),
        _ => normalize_opus47_clean_probe_mode(&settings.opus47_clean_probe_mode),
    }
}

pub fn effective_opus47_plain_stabilization_mode(settings: &RuntimeSettings) -> String {
    if settings.opus47_detection_profile == "cc_max_like" {
        "off".to_string()
    } else {
        normalize_opus47_plain_stabilization_mode(&settings.opus47_plain_stabilization_mode)
    }
}

pub fn effective_opus47_antml_probe_compat(settings: &RuntimeSettings) -> String {
    if settings.opus47_detection_profile == "cc_max_like" {
        "clarify".to_string()
    } else {
        normalize_opus47_antml_probe_compat(&settings.opus47_antml_probe_compat)
    }
}

pub fn effective_opus46_detection_profile(settings: &RuntimeSettings) -> String {
    normalize_model46_detection_profile(&settings.opus46_detection_profile)
}

pub fn effective_opus46_antml_probe_compat(settings: &RuntimeSettings) -> String {
    if settings.opus46_detection_profile == "cc_max_like" {
        "clarify".to_string()
    } else {
        normalize_opus47_antml_probe_compat(&settings.opus46_antml_probe_compat)
    }
}

pub fn effective_opus46_diagnostics_enabled(settings: &RuntimeSettings) -> bool {
    settings.opus46_diagnostics_enabled
}

pub fn effective_sonnet46_detection_profile(settings: &RuntimeSettings) -> String {
    normalize_model46_detection_profile(&settings.sonnet46_detection_profile)
}

pub fn effective_sonnet46_antml_probe_compat(settings: &RuntimeSettings) -> String {
    if settings.sonnet46_detection_profile == "cc_max_like" {
        "clarify".to_string()
    } else {
        normalize_opus47_antml_probe_compat(&settings.sonnet46_antml_probe_compat)
    }
}

pub fn effective_sonnet46_diagnostics_enabled(settings: &RuntimeSettings) -> bool {
    settings.sonnet46_diagnostics_enabled
}

fn any_cc_max_like_profile(settings: &RuntimeSettings) -> bool {
    settings.opus47_detection_profile == "cc_max_like"
        || settings.opus46_detection_profile == "cc_max_like"
        || settings.sonnet46_detection_profile == "cc_max_like"
}

pub fn effective_compat_usage_shape(settings: &RuntimeSettings) -> String {
    if any_cc_max_like_profile(settings) {
        "anthropic".to_string()
    } else {
        normalize_compat_usage_shape(&settings.compat_usage_shape)
    }
}

pub fn effective_compat_thinking_model(settings: &RuntimeSettings) -> String {
    if any_cc_max_like_profile(settings) {
        "native".to_string()
    } else {
        normalize_compat_thinking_model(&settings.compat_thinking_model)
    }
}

pub fn effective_compat_models_shape(settings: &RuntimeSettings) -> String {
    if any_cc_max_like_profile(settings) {
        "aggregator".to_string()
    } else {
        normalize_compat_models_shape(&settings.compat_models_shape)
    }
}

pub fn effective_opus47_detection_profile(settings: &RuntimeSettings) -> String {
    normalize_opus47_detection_profile(&settings.opus47_detection_profile)
}

pub fn effective_opus47_signed_thinking_preservation(settings: &RuntimeSettings) -> String {
    normalize_opus47_signed_thinking_preservation(&settings.opus47_signed_thinking_preservation)
}

pub fn effective_opus47_short_thinking_experiment(settings: &RuntimeSettings) -> String {
    normalize_opus47_short_thinking_experiment(&settings.opus47_short_thinking_experiment)
}

pub fn effective_opus47_diagnostics_enabled(settings: &RuntimeSettings) -> bool {
    settings.opus47_diagnostics_enabled
}

pub fn normalize_compat_usage_shape(shape: &str) -> String {
    if shape.trim().eq_ignore_ascii_case("flat") {
        "flat".to_string()
    } else {
        "anthropic".to_string()
    }
}

pub fn normalize_compat_thinking_model(mode: &str) -> String {
    if mode.trim().eq_ignore_ascii_case("plain_text") {
        "plain_text".to_string()
    } else {
        "native".to_string()
    }
}

pub fn normalize_compat_models_shape(shape: &str) -> String {
    if shape.trim().eq_ignore_ascii_case("aggregator") {
        "aggregator".to_string()
    } else {
        "anthropic".to_string()
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

pub fn same_account_retry_rule_matches(
    rule: &SameAccountRetryRule,
    status: u16,
    reason: Option<&str>,
) -> bool {
    if !rule.enabled || rule.attempts == 0 {
        return false;
    }
    if !status_pattern_matches(&rule.status, status) {
        return false;
    }
    match rule
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(expected) => reason
            .map(|actual| actual.eq_ignore_ascii_case(expected))
            .unwrap_or(false),
        None => true,
    }
}

pub fn matching_same_account_retry_rule<'a>(
    rules: &'a [SameAccountRetryRule],
    status: u16,
    reason: Option<&str>,
) -> Option<&'a SameAccountRetryRule> {
    rules
        .iter()
        .find(|rule| same_account_retry_rule_matches(rule, status, reason))
}

fn validate_same_account_retry_rules(rules: &[SameAccountRetryRule]) -> anyhow::Result<()> {
    if rules.len() > 50 {
        anyhow::bail!("sameAccountRetryRules 不能超过 50 条");
    }

    for (idx, rule) in rules.iter().enumerate() {
        validate_status_pattern(&rule.status)
            .map_err(|err| anyhow::anyhow!("sameAccountRetryRules[{}].status {}", idx, err))?;
        if rule.attempts > 10 {
            anyhow::bail!(
                "sameAccountRetryRules[{}].attempts 必须在 0..10 范围内",
                idx
            );
        }
        if !(100..=60_000).contains(&rule.delay_ms) {
            anyhow::bail!(
                "sameAccountRetryRules[{}].delayMs 必须在 100..60000 范围内",
                idx
            );
        }
        if let Some(reason) = &rule.reason {
            if reason.len() > 128 {
                anyhow::bail!("sameAccountRetryRules[{}].reason 不能超过 128 字符", idx);
            }
        }
    }
    Ok(())
}

fn validate_status_pattern(pattern: &str) -> anyhow::Result<()> {
    if pattern.trim().is_empty() {
        anyhow::bail!("不能为空");
    }
    for part in pattern.split(',') {
        let part = part.trim();
        if part.is_empty() {
            anyhow::bail!("包含空片段");
        }
        if let Some((start, end)) = part.split_once('-') {
            let start = parse_status_code(start.trim())?;
            let end = parse_status_code(end.trim())?;
            if start > end {
                anyhow::bail!("范围起点不能大于终点");
            }
        } else {
            parse_status_code(part)?;
        }
    }
    Ok(())
}

fn status_pattern_matches(pattern: &str, status: u16) -> bool {
    pattern.split(',').any(|part| {
        let part = part.trim();
        if let Some((start, end)) = part.split_once('-') {
            match (
                parse_status_code(start.trim()),
                parse_status_code(end.trim()),
            ) {
                (Ok(start), Ok(end)) => start <= status && status <= end,
                _ => false,
            }
        } else {
            parse_status_code(part)
                .map(|expected| expected == status)
                .unwrap_or(false)
        }
    })
}

fn parse_status_code(value: &str) -> anyhow::Result<u16> {
    let status = value
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("状态码必须是数字"))?;
    if !(100..=599).contains(&status) {
        anyhow::bail!("状态码必须在 100..599 范围内");
    }
    Ok(status)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(status: &str, reason: Option<&str>) -> SameAccountRetryRule {
        SameAccountRetryRule {
            enabled: true,
            status: status.to_string(),
            reason: reason.map(str::to_string),
            attempts: 2,
            delay_ms: 1_500,
            respect_retry_after: true,
        }
    }

    #[test]
    fn same_account_retry_rule_matches_status_and_reason() {
        let capacity = rule("429", Some("INSUFFICIENT_MODEL_CAPACITY"));
        assert!(same_account_retry_rule_matches(
            &capacity,
            429,
            Some("INSUFFICIENT_MODEL_CAPACITY")
        ));
        assert!(!same_account_retry_rule_matches(
            &capacity,
            429,
            Some("OTHER")
        ));
        assert!(!same_account_retry_rule_matches(&capacity, 500, None));
    }

    #[test]
    fn same_account_retry_rule_supports_ranges_and_lists() {
        let transient = rule("408,500-599", None);
        assert!(same_account_retry_rule_matches(&transient, 408, None));
        assert!(same_account_retry_rule_matches(&transient, 503, None));
        assert!(!same_account_retry_rule_matches(&transient, 429, None));
    }

    #[test]
    fn same_account_retry_rule_validation_rejects_bad_status() {
        let invalid = rule("599-500", None);
        assert!(validate_same_account_retry_rules(&[invalid]).is_err());
    }

    #[test]
    fn credential_policy_turbo_defaults_and_validation() {
        let default_policy = CredentialPolicy::default();
        assert_eq!(default_policy.effective_turbo_mode(), "off");
        assert_eq!(default_policy.effective_turbo_fanout(), 1);
        assert!(default_policy.uses_default_policy());

        let race = CredentialPolicy {
            turbo_mode: "race".to_string(),
            turbo_fanout: 3,
            ..CredentialPolicy::default()
        };
        assert!(race.validate().is_ok());
        assert_eq!(race.effective_turbo_mode(), "race");
        assert_eq!(race.effective_turbo_fanout(), 3);
        assert!(!race.uses_default_policy());

        let invalid = CredentialPolicy {
            turbo_mode: "race".to_string(),
            turbo_fanout: 1,
            ..CredentialPolicy::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn opus47_profile_defaults_and_normalization() {
        let settings = RuntimeSettings::from_config(&Config::default());
        assert_eq!(settings.opus47_detection_profile, "normal");
        assert_eq!(settings.opus47_signed_thinking_preservation, "off");
        assert_eq!(settings.opus47_short_thinking_experiment, "off");
        assert_eq!(settings.opus46_detection_profile, "normal");
        assert_eq!(settings.opus46_antml_probe_compat, "off");
        assert!(settings.opus46_diagnostics_enabled);
        assert!(!settings.opus46_raw_debug_enabled);
        assert_eq!(settings.opus46_raw_debug_max_chars, 20_000);
        assert_eq!(settings.sonnet46_detection_profile, "normal");
        assert_eq!(settings.sonnet46_antml_probe_compat, "off");
        assert!(settings.sonnet46_diagnostics_enabled);
        assert!(!settings.sonnet46_raw_debug_enabled);
        assert_eq!(settings.sonnet46_raw_debug_max_chars, 20_000);
        assert!(!settings.prompt_dump_enabled);
        assert_eq!(settings.prompt_dump_dir, "/app/config/prompt-dumps");
        assert_eq!(settings.prompt_dump_max_bytes, 2_000_000);
        assert_eq!(
            settings.prompt_dump_models,
            "claude-opus-4-6,claude-opus-4-7,claude-opus-4-8,claude-sonnet-4-6,claude-sonnet-5"
        );
        assert!(!settings.message_pruning_enabled);
        assert_eq!(settings.message_pruning_max_request_bytes, 629_760);
        assert_eq!(settings.message_pruning_keep_recent_messages, 2);
        assert_eq!(settings.message_pruning_max_history_entry_bytes, 300_000);
        assert_eq!(settings.message_pruning_max_truncated_content_bytes, 50_000);
        assert_eq!(settings.target_cache_reuse_ratio, 0.0);
        assert_eq!(settings.virtual_cache_context_shrink_reset_ratio, 0.7);
        assert!(!settings.virtual_cache_haiku_input_only_enabled);
        assert_eq!(
            normalize_opus47_detection_profile("cc-max-like"),
            "cc_max_like"
        );
        assert_eq!(
            normalize_opus47_detection_profile("clean-probe-debug"),
            "clean_probe_debug"
        );
        assert_eq!(
            normalize_model46_detection_profile("cc-max-like"),
            "cc_max_like"
        );
        assert_eq!(
            normalize_model46_detection_profile("clean-probe-debug"),
            "normal"
        );
        assert_eq!(
            normalize_opus47_signed_thinking_preservation("history-experiment"),
            "history_experiment"
        );
        assert_eq!(
            normalize_opus47_short_thinking_experiment("adaptive-high"),
            "adaptive_high"
        );
        assert_eq!(
            normalize_prompt_dump_models(" Claude-Opus-4-7, claude-sonnet-4-6 "),
            "claude-opus-4-7,claude-sonnet-4-6"
        );
    }

    #[test]
    fn prompt_dump_settings_validate_range() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.prompt_dump_max_bytes = 9_999;
        assert!(settings.validate().is_err());

        settings.prompt_dump_max_bytes = 50_000_001;
        assert!(settings.validate().is_err());

        settings.prompt_dump_max_bytes = 10_000;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn message_pruning_settings_validate_range() {
        let mut settings = RuntimeSettings::from_config(&Config::default());

        settings.message_pruning_max_request_bytes = 99_999;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_request_bytes = 5_000_001;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_request_bytes = 629_760;

        settings.message_pruning_keep_recent_messages = 0;
        assert!(settings.validate().is_err());
        settings.message_pruning_keep_recent_messages = 201;
        assert!(settings.validate().is_err());
        settings.message_pruning_keep_recent_messages = 2;

        settings.message_pruning_max_history_entry_bytes = 9_999;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_history_entry_bytes = 2_000_001;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_history_entry_bytes = 300_000;

        settings.message_pruning_max_truncated_content_bytes = 999;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_truncated_content_bytes = 500_001;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_truncated_content_bytes = 300_001;
        assert!(settings.validate().is_err());
        settings.message_pruning_max_truncated_content_bytes = 50_000;

        assert!(settings.validate().is_ok());
    }

    #[test]
    fn global_max_concurrent_limit_controls_global_concurrent_validation() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.global_max_concurrent = 513;
        settings.global_max_concurrent_limit = 512;
        assert!(settings.validate().is_err());

        settings.global_max_concurrent_limit = 1024;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn target_cache_reuse_ratio_validates_percent_fraction() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.target_cache_reuse_ratio = 0.95;
        assert!(settings.validate().is_ok());

        settings.target_cache_reuse_ratio = 1.01;
        assert!(settings.validate().is_err());

        settings.target_cache_reuse_ratio = -0.01;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn context_shrink_reset_ratio_validates_percent_fraction() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.virtual_cache_context_shrink_reset_ratio = 0.2;
        assert!(settings.validate().is_ok());

        settings.virtual_cache_context_shrink_reset_ratio = 0.0;
        assert!(settings.validate().is_ok());

        settings.virtual_cache_context_shrink_reset_ratio = 1.01;
        assert!(settings.validate().is_err());

        settings.virtual_cache_context_shrink_reset_ratio = -0.01;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn cc_max_like_profile_applies_effective_presets() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus47_detection_profile = "cc_max_like".to_string();
        settings.opus47_clean_probe_mode = "clean".to_string();
        settings.opus47_plain_stabilization_mode = "adaptive_high".to_string();
        settings.opus47_antml_probe_compat = "off".to_string();
        settings.compat_usage_shape = "anthropic".to_string();
        settings.compat_thinking_model = "plain_text".to_string();
        settings.compat_models_shape = "anthropic".to_string();

        assert_eq!(effective_opus47_clean_probe_mode(&settings), "off");
        assert_eq!(effective_opus47_plain_stabilization_mode(&settings), "off");
        assert_eq!(effective_opus47_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
        assert_eq!(effective_compat_thinking_model(&settings), "native");
        assert_eq!(effective_compat_models_shape(&settings), "aggregator");
    }

    #[test]
    fn opus47_effective_settings_follow_granular_fields_without_presets() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus47_detection_profile = "cc_max_like".to_string();
        settings.opus47_signed_thinking_preservation = "off".to_string();
        settings.opus47_antml_probe_compat = "off".to_string();
        settings.compat_usage_shape = "anthropic".to_string();
        settings.compat_models_shape = "anthropic".to_string();
        settings.opus47_diagnostics_enabled = true;
        settings.prompt_dump_enabled = true;

        assert_eq!(effective_opus47_detection_profile(&settings), "cc_max_like");
        assert_eq!(
            effective_opus47_signed_thinking_preservation(&settings),
            "off"
        );
        assert_eq!(effective_opus47_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
        assert_eq!(effective_compat_models_shape(&settings), "aggregator");
        assert!(effective_opus47_diagnostics_enabled(&settings));
        assert!(settings.prompt_dump_enabled);
    }

    #[test]
    fn model46_effective_settings_follow_granular_fields_without_presets() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus46_detection_profile = "cc_max_like".to_string();
        settings.opus46_antml_probe_compat = "off".to_string();
        settings.sonnet46_detection_profile = "normal".to_string();
        settings.compat_usage_shape = "anthropic".to_string();
        settings.compat_thinking_model = "plain_text".to_string();
        settings.compat_models_shape = "anthropic".to_string();
        settings.prompt_dump_enabled = false;

        assert_eq!(effective_opus46_detection_profile(&settings), "cc_max_like");
        assert_eq!(effective_opus46_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_sonnet46_detection_profile(&settings), "normal");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
        assert_eq!(effective_compat_thinking_model(&settings), "native");
        assert_eq!(effective_compat_models_shape(&settings), "aggregator");
        assert!(!settings.prompt_dump_enabled);

        settings.opus46_detection_profile = "normal".to_string();
        settings.sonnet46_detection_profile = "cc_max_like".to_string();
        settings.compat_usage_shape = "anthropic".to_string();
        settings.compat_thinking_model = "plain_text".to_string();
        settings.compat_models_shape = "anthropic".to_string();
        settings.prompt_dump_enabled = true;

        assert_eq!(effective_opus46_detection_profile(&settings), "normal");
        assert_eq!(
            effective_sonnet46_detection_profile(&settings),
            "cc_max_like"
        );
        assert_eq!(effective_opus46_antml_probe_compat(&settings), "off");
        assert_eq!(effective_sonnet46_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
        assert_eq!(effective_compat_thinking_model(&settings), "native");
        assert_eq!(effective_compat_models_shape(&settings), "aggregator");
        assert!(settings.prompt_dump_enabled);
    }

    #[test]
    fn model46_cc_max_like_applies_compat_shape() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus46_detection_profile = "cc_max_like".to_string();
        settings.sonnet46_detection_profile = "normal".to_string();
        settings.compat_usage_shape = "anthropic".to_string();
        settings.compat_thinking_model = "plain_text".to_string();
        settings.compat_models_shape = "anthropic".to_string();

        assert_eq!(effective_opus46_detection_profile(&settings), "cc_max_like");
        assert_eq!(effective_opus46_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
        assert_eq!(effective_compat_thinking_model(&settings), "native");
        assert_eq!(effective_compat_models_shape(&settings), "aggregator");

        settings.opus46_detection_profile = "normal".to_string();
        settings.sonnet46_detection_profile = "cc_max_like".to_string();

        assert_eq!(
            effective_sonnet46_detection_profile(&settings),
            "cc_max_like"
        );
        assert_eq!(effective_sonnet46_antml_probe_compat(&settings), "clarify");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
    }

    #[test]
    fn clean_probe_debug_profile_keeps_clean_probe_effective() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus47_detection_profile = "clean_probe_debug".to_string();
        settings.opus47_clean_probe_mode = "off".to_string();

        assert_eq!(effective_opus47_clean_probe_mode(&settings), "clean");
        assert_eq!(effective_compat_usage_shape(&settings), "anthropic");
    }
}
