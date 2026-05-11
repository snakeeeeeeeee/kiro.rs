use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::kiro::settings::{RuntimeSettings, normalize_virtual_cache_ttl};
use crate::token;

use super::converter::extract_session_id;
use super::types::MessagesRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTtl {
    FiveMinutes,
    OneHour,
}

impl CacheTtl {
    pub fn from_str(value: &str) -> Self {
        if value == "1h" {
            Self::OneHour
        } else {
            Self::FiveMinutes
        }
    }

    pub fn from_runtime_default(settings: &RuntimeSettings) -> Self {
        Self::from_str(&normalize_virtual_cache_ttl(
            &settings.virtual_cache_default_ttl,
        ))
    }

    fn duration(self) -> Duration {
        match self {
            Self::FiveMinutes => Duration::minutes(5),
            Self::OneHour => Duration::hours(1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VirtualUsageInput {
    pub credential_id: u64,
    pub model: String,
    pub session_key: String,
    pub observed_total_input_tokens: i32,
    pub estimated_uncached_input_tokens: Option<i32>,
    pub output_tokens: i32,
    pub creation_ttl: CacheTtl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicUsage {
    pub input_tokens: i32,
    pub cache_read_input_tokens: i32,
    pub cache_creation_input_tokens: i32,
    pub ephemeral_5m_input_tokens: i32,
    pub ephemeral_1h_input_tokens: i32,
    pub output_tokens: i32,
    include_cache_fields: bool,
}

impl AnthropicUsage {
    pub fn simple(input_tokens: i32, output_tokens: i32) -> Self {
        Self {
            input_tokens: input_tokens.max(0),
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            ephemeral_5m_input_tokens: 0,
            ephemeral_1h_input_tokens: 0,
            output_tokens: output_tokens.max(0),
            include_cache_fields: false,
        }
    }

    pub fn to_json(&self) -> Value {
        if self.include_cache_fields {
            json!({
                "input_tokens": self.input_tokens,
                "cache_read_input_tokens": self.cache_read_input_tokens,
                "cache_creation_input_tokens": self.cache_creation_input_tokens,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": self.ephemeral_5m_input_tokens,
                    "ephemeral_1h_input_tokens": self.ephemeral_1h_input_tokens
                },
                "output_tokens": self.output_tokens
            })
        } else {
            json!({
                "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens
            })
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct LedgerKey {
    credential_id: u64,
    model: String,
    session_key: String,
}

#[derive(Debug, Clone, Default)]
struct LedgerEntry {
    cached_5m_tokens: i32,
    cached_5m_expires_at: Option<DateTime<Utc>>,
    cached_1h_tokens: i32,
    cached_1h_expires_at: Option<DateTime<Utc>>,
    last_observed_input_tokens: i32,
    turn_count: u64,
}

#[derive(Default)]
pub struct VirtualCacheUsageManager {
    ledgers: Mutex<HashMap<LedgerKey, LedgerEntry>>,
}

#[derive(Debug, Clone)]
pub struct PendingVirtualUsage {
    key: Option<LedgerKey>,
    usage: AnthropicUsage,
    creation_ttl: CacheTtl,
    creation_tokens: i32,
    observed_total_input_tokens: i32,
}

impl PendingVirtualUsage {
    pub fn usage(&self) -> &AnthropicUsage {
        &self.usage
    }
}

impl VirtualCacheUsageManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build_usage(
        &self,
        settings: &RuntimeSettings,
        input: VirtualUsageInput,
    ) -> AnthropicUsage {
        let pending = self.preview_usage(settings, input);
        let usage = pending.usage.clone();
        self.commit_usage(pending);
        usage
    }

    pub fn preview_usage(
        &self,
        settings: &RuntimeSettings,
        input: VirtualUsageInput,
    ) -> PendingVirtualUsage {
        self.preview_usage_at(settings, input, Utc::now())
    }

    pub fn commit_usage(&self, pending: PendingVirtualUsage) {
        self.commit_usage_at(pending, Utc::now());
    }

    fn preview_usage_at(
        &self,
        settings: &RuntimeSettings,
        input: VirtualUsageInput,
        now: DateTime<Utc>,
    ) -> PendingVirtualUsage {
        let observed_total = input.observed_total_input_tokens.max(0);
        if !settings.virtual_cache_usage_enabled || observed_total == 0 {
            return PendingVirtualUsage {
                key: None,
                usage: AnthropicUsage::simple(observed_total, input.output_tokens),
                creation_ttl: input.creation_ttl,
                creation_tokens: 0,
                observed_total_input_tokens: observed_total,
            };
        }

        let key = LedgerKey {
            credential_id: input.credential_id,
            model: input.model.clone(),
            session_key: input.session_key.clone(),
        };

        let mut entry = self.ledgers.lock().get(&key).cloned().unwrap_or_default();
        expire_entry(&mut entry, now);

        let read_tokens = entry
            .cached_5m_tokens
            .saturating_add(entry.cached_1h_tokens);
        let uncached = compute_uncached_tokens(settings, &input, observed_total);

        let creation_tokens =
            compute_creation_tokens(settings, &input, &entry, observed_total, uncached);

        let (ephemeral_5m_input_tokens, ephemeral_1h_input_tokens) = match input.creation_ttl {
            CacheTtl::FiveMinutes => (creation_tokens, 0),
            CacheTtl::OneHour => (0, creation_tokens),
        };

        PendingVirtualUsage {
            key: Some(key),
            usage: AnthropicUsage {
                input_tokens: uncached,
                cache_read_input_tokens: read_tokens,
                cache_creation_input_tokens: creation_tokens,
                ephemeral_5m_input_tokens,
                ephemeral_1h_input_tokens,
                output_tokens: input.output_tokens.max(0),
                include_cache_fields: true,
            },
            creation_ttl: input.creation_ttl,
            creation_tokens,
            observed_total_input_tokens: observed_total,
        }
    }

    fn commit_usage_at(&self, pending: PendingVirtualUsage, now: DateTime<Utc>) {
        let Some(key) = pending.key else {
            return;
        };

        let mut ledgers = self.ledgers.lock();
        let entry = ledgers.entry(key).or_default();
        expire_entry(entry, now);

        match pending.creation_ttl {
            CacheTtl::FiveMinutes => {
                entry.cached_5m_tokens = entry
                    .cached_5m_tokens
                    .saturating_add(pending.creation_tokens);
                entry.cached_5m_expires_at = Some(now + pending.creation_ttl.duration());
            }
            CacheTtl::OneHour => {
                entry.cached_1h_tokens = entry
                    .cached_1h_tokens
                    .saturating_add(pending.creation_tokens);
                entry.cached_1h_expires_at = Some(now + pending.creation_ttl.duration());
            }
        }
        entry.last_observed_input_tokens = entry
            .last_observed_input_tokens
            .max(pending.observed_total_input_tokens);
        entry.turn_count = entry.turn_count.saturating_add(1);
    }
}

fn compute_uncached_tokens(
    settings: &RuntimeSettings,
    input: &VirtualUsageInput,
    observed_total: i32,
) -> i32 {
    if settings.virtual_cache_input_mode == "estimated_user_delta" {
        let estimated = input
            .estimated_uncached_input_tokens
            .unwrap_or(settings.virtual_cache_uncached_input_tokens as i32)
            .max(1);
        estimated
            .clamp(
                settings.virtual_cache_min_input_tokens as i32,
                settings.virtual_cache_max_input_tokens as i32,
            )
            .min(observed_total)
            .max(1)
    } else {
        settings
            .virtual_cache_uncached_input_tokens
            .min(observed_total as u32) as i32
    }
}

fn compute_creation_tokens(
    settings: &RuntimeSettings,
    input: &VirtualUsageInput,
    entry: &LedgerEntry,
    observed_total: i32,
    uncached: i32,
) -> i32 {
    if entry.turn_count == 0 {
        return observed_total
            .saturating_sub(uncached)
            .max(settings.virtual_cache_warmup_tokens as i32);
    }

    let delta = observed_total.saturating_sub(entry.last_observed_input_tokens);
    if settings.virtual_cache_creation_mode != "dynamic" {
        return delta
            .max(settings.virtual_cache_min_creation_tokens as i32)
            .clamp(0, settings.virtual_cache_max_creation_tokens as i32);
    }

    let base = delta
        .max(input.estimated_uncached_input_tokens.unwrap_or_default())
        .max(settings.virtual_cache_min_creation_tokens as i32);
    let output_component = input.output_tokens.max(0).min(2_000) / 2;
    let mut creation = base.saturating_add(output_component);
    creation = apply_creation_jitter(settings, input, entry.turn_count, creation);
    creation = creation.clamp(
        settings.virtual_cache_min_creation_tokens as i32,
        settings.virtual_cache_max_creation_tokens as i32,
    );

    let next_turn = entry.turn_count.saturating_add(1);
    if should_apply_burst(settings, next_turn) {
        creation = creation.saturating_add(deterministic_range(
            input,
            next_turn,
            "burst",
            settings.virtual_cache_burst_min_tokens,
            settings.virtual_cache_burst_max_tokens,
        ));
        let burst_ceiling = settings
            .virtual_cache_max_creation_tokens
            .max(settings.virtual_cache_burst_max_tokens) as i32;
        creation = creation.clamp(
            settings.virtual_cache_min_creation_tokens as i32,
            burst_ceiling,
        );
    }

    creation.max(0)
}

fn apply_creation_jitter(
    settings: &RuntimeSettings,
    input: &VirtualUsageInput,
    turn_count: u64,
    value: i32,
) -> i32 {
    if value <= 0 || settings.virtual_cache_creation_jitter_ratio <= 0.0 {
        return value;
    }

    let ratio = settings.virtual_cache_creation_jitter_ratio.clamp(0.0, 1.0);
    let spread = ((value as f64) * ratio).round() as i32;
    if spread <= 0 {
        return value;
    }

    let offset = deterministic_range_i32(input, turn_count, "jitter", -spread, spread);
    value.saturating_add(offset)
}

fn should_apply_burst(settings: &RuntimeSettings, next_turn: u64) -> bool {
    settings.virtual_cache_burst_every_turns > 0
        && settings.virtual_cache_burst_max_tokens > 0
        && next_turn % settings.virtual_cache_burst_every_turns as u64 == 0
}

fn deterministic_range(
    input: &VirtualUsageInput,
    turn_count: u64,
    salt: &str,
    min: u32,
    max: u32,
) -> i32 {
    if max <= min {
        return min as i32;
    }
    deterministic_range_i32(input, turn_count, salt, min as i32, max as i32)
}

fn deterministic_range_i32(
    input: &VirtualUsageInput,
    turn_count: u64,
    salt: &str,
    min: i32,
    max: i32,
) -> i32 {
    if max <= min {
        return min;
    }

    let hash = stable_hash(input, turn_count, salt);
    let span = (max as i64 - min as i64 + 1) as u64;
    min.saturating_add((hash % span) as i32)
}

fn stable_hash(input: &VirtualUsageInput, turn_count: u64, salt: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(input.credential_id.to_le_bytes());
    hasher.update(input.model.as_bytes());
    hasher.update(input.session_key.as_bytes());
    hasher.update(turn_count.to_le_bytes());
    hasher.update(input.observed_total_input_tokens.to_le_bytes());
    hasher.update(input.output_tokens.to_le_bytes());
    hasher.update(salt.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

fn expire_entry(entry: &mut LedgerEntry, now: DateTime<Utc>) {
    if entry
        .cached_5m_expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        entry.cached_5m_tokens = 0;
        entry.cached_5m_expires_at = None;
    }
    if entry
        .cached_1h_expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        entry.cached_1h_tokens = 0;
        entry.cached_1h_expires_at = None;
    }
}

pub fn estimate_latest_user_input_tokens(req: &MessagesRequest) -> i32 {
    req.messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| estimate_content_tokens(&message.content))
        .unwrap_or(0)
        .max(1)
}

fn estimate_content_tokens(value: &Value) -> i32 {
    match value {
        Value::String(text) => token::count_tokens(text) as i32,
        Value::Array(items) => items.iter().map(estimate_content_tokens).sum(),
        Value::Object(map) => {
            let mut total = 0;
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                total += token::count_tokens(text) as i32;
            }
            if let Some(content) = map.get("content") {
                total += estimate_content_tokens(content);
            }
            if let Some(input) = map.get("input") {
                total += estimate_json_tokens(input);
            }
            total
        }
        _ => 0,
    }
}

fn estimate_json_tokens(value: &Value) -> i32 {
    serde_json::to_string(value)
        .map(|text| token::count_tokens(&text) as i32)
        .unwrap_or(0)
}

pub fn session_key_for_request(req: &MessagesRequest, model: &str, fallback_scope: &str) -> String {
    if let Some(user_id) = req
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.user_id.as_ref())
    {
        if let Some(session_id) = extract_session_id(user_id) {
            return session_id;
        }
        let trimmed = user_id.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if fallback_scope == "none" {
        format!("fallback:none:{}", uuid::Uuid::new_v4())
    } else {
        format!("fallback:model:{}", model)
    }
}

pub fn request_cache_ttl(req: &MessagesRequest, default_ttl: CacheTtl) -> CacheTtl {
    let mut ttl = default_ttl;

    if let Some(tools) = &req.tools {
        for tool in tools {
            if let Some(cache_control) = &tool.cache_control {
                ttl = cache_control_ttl(cache_control.ttl.as_deref(), default_ttl);
            }
        }
    }

    if let Some(system) = &req.system {
        for message in system {
            if let Some(cache_control) = &message.cache_control {
                ttl = cache_control_ttl(cache_control.ttl.as_deref(), default_ttl);
            }
        }
    }

    for message in &req.messages {
        collect_content_ttl(&message.content, &mut ttl, default_ttl);
    }

    ttl
}

fn collect_content_ttl(value: &Value, ttl: &mut CacheTtl, default_ttl: CacheTtl) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_content_ttl(item, ttl, default_ttl);
            }
        }
        Value::Object(map) => {
            if let Some(cache_control) = map.get("cache_control") {
                *ttl = value_cache_control_ttl(cache_control, default_ttl);
            }
            if let Some(content) = map.get("content") {
                collect_content_ttl(content, ttl, default_ttl);
            }
        }
        _ => {}
    }
}

fn value_cache_control_ttl(value: &Value, default_ttl: CacheTtl) -> CacheTtl {
    let ttl = value
        .as_object()
        .and_then(|map| map.get("ttl"))
        .and_then(Value::as_str);
    cache_control_ttl(ttl, default_ttl)
}

fn cache_control_ttl(ttl: Option<&str>, default_ttl: CacheTtl) -> CacheTtl {
    match ttl {
        Some("1h") => CacheTtl::OneHour,
        Some("5m") | None => CacheTtl::FiveMinutes,
        Some(_) => default_ttl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> RuntimeSettings {
        RuntimeSettings {
            global_max_concurrent: 32,
            per_account_default_max_concurrent: 3,
            queue_max_size: 128,
            queue_timeout_ms: 30_000,
            per_account_default_rpm: 0,
            global_rpm: 0,
            rate_limit_cooldown_ms: 60_000,
            transient_cooldown_ms: 10_000,
            max_retry_accounts: 3,
            model_capacity_cooldown_ms: 10_000,
            token_auto_refresh_enabled: true,
            token_auto_refresh_interval_secs: 300,
            token_auto_refresh_window_secs: 1_800,
            load_balancing_mode: "priority".to_string(),
            virtual_cache_usage_enabled: true,
            virtual_cache_default_ttl: "5m".to_string(),
            virtual_cache_uncached_input_tokens: 1,
            virtual_cache_input_mode: "fixed".to_string(),
            virtual_cache_min_input_tokens: 8,
            virtual_cache_max_input_tokens: 96,
            virtual_cache_warmup_tokens: 18_000,
            virtual_cache_min_creation_tokens: 128,
            virtual_cache_max_creation_tokens: 1_200,
            virtual_cache_creation_mode: "fixed".to_string(),
            virtual_cache_creation_jitter_ratio: 0.25,
            virtual_cache_burst_every_turns: 7,
            virtual_cache_burst_min_tokens: 1_500,
            virtual_cache_burst_max_tokens: 3_000,
            virtual_cache_fallback_scope: "model".to_string(),
            dynamic_proxy_enabled: false,
            dynamic_proxy_provider: "novproxy".to_string(),
            dynamic_proxy_protocol: "http".to_string(),
            dynamic_proxy_host: "us.novproxy.io".to_string(),
            dynamic_proxy_port: 1000,
            dynamic_proxy_username_template:
                "nfgr68136-region-{region}-st-{state}-sid-{sid}-t-{ttl}".to_string(),
            dynamic_proxy_password: String::new(),
            dynamic_proxy_region: "US".to_string(),
            dynamic_proxy_state: "New Jersey".to_string(),
            dynamic_proxy_ttl_minutes: 120,
            dynamic_proxy_renew_before_ms: 900_000,
            dynamic_proxy_verify_url: "https://ipinfo.io/json".to_string(),
            dynamic_proxy_max_bind_retries: 3,
            dynamic_proxy_auto_bind_new_accounts: false,
            dynamic_proxy_worker_interval_ms: 60_000,
            dynamic_proxy_worker_batch_size: 20,
            dynamic_proxy_worker_concurrency: 3,
        }
    }

    fn input(session: &str, observed: i32, ttl: CacheTtl) -> VirtualUsageInput {
        VirtualUsageInput {
            credential_id: 1,
            model: "claude-sonnet-4-5-20250929".to_string(),
            session_key: session.to_string(),
            observed_total_input_tokens: observed,
            estimated_uncached_input_tokens: None,
            output_tokens: 7,
            creation_ttl: ttl,
        }
    }

    #[test]
    fn same_session_accumulates_cache_read() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let first = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        assert_eq!(first.input_tokens, 1);
        assert_eq!(first.cache_read_input_tokens, 0);
        assert_eq!(first.cache_creation_input_tokens, 18_000);

        let second =
            manager.build_usage(&settings, input("session-a", 1100, CacheTtl::FiveMinutes));
        assert_eq!(second.cache_read_input_tokens, 18_000);
        assert_eq!(second.cache_creation_input_tokens, 128);
    }

    #[test]
    fn sessions_models_and_credentials_are_isolated() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let _ = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));

        let other_session =
            manager.build_usage(&settings, input("session-b", 1000, CacheTtl::FiveMinutes));
        assert_eq!(other_session.cache_read_input_tokens, 0);

        let other_model = manager.build_usage(
            &settings,
            VirtualUsageInput {
                model: "claude-opus-4-7".to_string(),
                ..input("session-a", 1000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(other_model.cache_read_input_tokens, 0);

        let other_credential = manager.build_usage(
            &settings,
            VirtualUsageInput {
                credential_id: 2,
                ..input("session-a", 1000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(other_credential.cache_read_input_tokens, 0);
    }

    #[test]
    fn one_hour_creation_uses_one_hour_bucket() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let usage = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::OneHour));
        assert_eq!(usage.ephemeral_5m_input_tokens, 0);
        assert_eq!(
            usage.ephemeral_1h_input_tokens,
            usage.cache_creation_input_tokens
        );
    }

    #[test]
    fn preview_does_not_accumulate_until_commit() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let first_preview =
            manager.preview_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        assert_eq!(first_preview.usage().cache_read_input_tokens, 0);

        let second_preview =
            manager.preview_usage(&settings, input("session-a", 1100, CacheTtl::FiveMinutes));
        assert_eq!(second_preview.usage().cache_read_input_tokens, 0);

        manager.commit_usage(first_preview);

        let after_commit =
            manager.preview_usage(&settings, input("session-a", 1100, CacheTtl::FiveMinutes));
        assert_eq!(after_commit.usage().cache_read_input_tokens, 18_000);
    }

    #[test]
    fn disabled_virtual_cache_returns_simple_usage_json() {
        let manager = VirtualCacheUsageManager::new();
        let mut settings = settings();
        settings.virtual_cache_usage_enabled = false;

        let usage = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        let json = usage.to_json();

        assert_eq!(json["input_tokens"], 1000);
        assert_eq!(json["output_tokens"], 7);
        assert!(json.get("cache_read_input_tokens").is_none());
        assert!(json.get("cache_creation_input_tokens").is_none());
        assert!(json.get("cache_creation").is_none());
    }

    #[test]
    fn estimated_user_delta_input_mode_uses_latest_user_estimate_with_clamps() {
        let manager = VirtualCacheUsageManager::new();
        let mut settings = settings();
        settings.virtual_cache_input_mode = "estimated_user_delta".to_string();
        settings.virtual_cache_min_input_tokens = 8;
        settings.virtual_cache_max_input_tokens = 96;

        let usage = manager.build_usage(
            &settings,
            VirtualUsageInput {
                estimated_uncached_input_tokens: Some(42),
                ..input("session-a", 1000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(usage.input_tokens, 42);

        let low = manager.build_usage(
            &settings,
            VirtualUsageInput {
                estimated_uncached_input_tokens: Some(2),
                ..input("session-b", 1000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(low.input_tokens, 8);

        let high = manager.build_usage(
            &settings,
            VirtualUsageInput {
                estimated_uncached_input_tokens: Some(300),
                ..input("session-c", 1000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(high.input_tokens, 96);
    }

    #[test]
    fn dynamic_creation_mode_varies_after_warmup() {
        let manager = VirtualCacheUsageManager::new();
        let mut settings = settings();
        settings.virtual_cache_creation_mode = "dynamic".to_string();
        settings.virtual_cache_creation_jitter_ratio = 0.25;
        settings.virtual_cache_burst_every_turns = 0;

        let _ = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        let second = manager.build_usage(
            &settings,
            VirtualUsageInput {
                estimated_uncached_input_tokens: Some(80),
                output_tokens: 350,
                ..input("session-a", 1050, CacheTtl::FiveMinutes)
            },
        );

        assert_ne!(second.cache_creation_input_tokens, 128);
        assert!(
            (settings.virtual_cache_min_creation_tokens as i32
                ..=settings.virtual_cache_max_creation_tokens as i32)
                .contains(&second.cache_creation_input_tokens)
        );
    }

    #[test]
    fn dynamic_burst_can_exceed_normal_creation_max() {
        let manager = VirtualCacheUsageManager::new();
        let mut settings = settings();
        settings.virtual_cache_creation_mode = "dynamic".to_string();
        settings.virtual_cache_creation_jitter_ratio = 0.0;
        settings.virtual_cache_burst_every_turns = 2;
        settings.virtual_cache_burst_min_tokens = 1_500;
        settings.virtual_cache_burst_max_tokens = 3_000;

        let _ = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        let burst = manager.build_usage(
            &settings,
            VirtualUsageInput {
                estimated_uncached_input_tokens: Some(80),
                output_tokens: 200,
                ..input("session-a", 1050, CacheTtl::FiveMinutes)
            },
        );

        assert!(
            burst.cache_creation_input_tokens > settings.virtual_cache_max_creation_tokens as i32
        );
        assert!(
            burst.cache_creation_input_tokens <= settings.virtual_cache_burst_max_tokens as i32
        );
    }

    #[test]
    fn ttl_buckets_expire_independently() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let now = Utc::now();

        let five_minute = manager.preview_usage_at(
            &settings,
            input("session-a", 1000, CacheTtl::FiveMinutes),
            now,
        );
        manager.commit_usage_at(five_minute, now);

        let one_hour = manager.preview_usage_at(
            &settings,
            input("session-a", 1100, CacheTtl::OneHour),
            now + Duration::seconds(1),
        );
        assert_eq!(one_hour.usage().cache_read_input_tokens, 18_000);
        manager.commit_usage_at(one_hour, now + Duration::seconds(1));

        let after_five_minutes = manager.preview_usage_at(
            &settings,
            input("session-a", 1200, CacheTtl::FiveMinutes),
            now + Duration::minutes(6),
        );
        assert_eq!(after_five_minutes.usage().cache_read_input_tokens, 128);
    }

    #[test]
    fn request_cache_ttl_uses_last_prompt_cache_control() {
        let request: MessagesRequest = serde_json::from_value(json!({
            "model": "claude-sonnet-4-5-20250929",
            "max_tokens": 100,
            "system": [
                {
                    "type": "text",
                    "text": "system",
                    "cache_control": { "type": "ephemeral", "ttl": "1h" }
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "hello",
                            "cache_control": { "type": "ephemeral", "ttl": "5m" }
                        },
                        {
                            "type": "text",
                            "text": "world",
                            "cache_control": { "type": "ephemeral", "ttl": "1h" }
                        }
                    ]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            request_cache_ttl(&request, CacheTtl::FiveMinutes),
            CacheTtl::OneHour
        );
    }
}
