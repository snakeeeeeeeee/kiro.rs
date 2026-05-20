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
    pub model: String,
    pub session_key: String,
    /// Upstream context size observed from Kiro. Used to detect context shrink/compression.
    pub observed_total_input_tokens: i32,
    /// Synthetic billing/accounting input size. Defaults to `observed_total_input_tokens`.
    pub accounting_total_input_tokens: Option<i32>,
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

    #[cfg(test)]
    pub fn to_json(&self) -> Value {
        self.to_json_with_shape("anthropic")
    }

    pub fn to_json_with_shape(&self, shape: &str) -> Value {
        if self.include_cache_fields {
            if shape == "flat" {
                json!({
                    "input_tokens": self.input_tokens,
                    "cache_read_input_tokens": self.cache_read_input_tokens,
                    "cache_creation_input_tokens": self.cache_creation_input_tokens,
                    "output_tokens": self.output_tokens
                })
            } else {
                json!({
                    "input_tokens": self.input_tokens,
                    "cache_read_input_tokens": self.cache_read_input_tokens,
                    "cache_creation_input_tokens": self.cache_creation_input_tokens,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": self.ephemeral_5m_input_tokens,
                        "ephemeral_1h_input_tokens": self.ephemeral_1h_input_tokens
                    },
                    "output_tokens": self.output_tokens,
                    "service_tier": "standard",
                    "inference_geo": "global"
                })
            }
        } else {
            json!({
                "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens,
                "service_tier": "standard",
                "inference_geo": "global"
            })
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct LedgerKey {
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
    last_accounting_input_tokens: i32,
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
    accounting_total_input_tokens: i32,
    reset_ledger: bool,
}

impl PendingVirtualUsage {
    pub fn usage(&self) -> &AnthropicUsage {
        &self.usage
    }

    pub fn observed_total_input_tokens(&self) -> i32 {
        self.observed_total_input_tokens
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

    pub fn preview_usage_without_context_shrink_reset(
        &self,
        settings: &RuntimeSettings,
        input: VirtualUsageInput,
    ) -> PendingVirtualUsage {
        self.preview_usage_at_with_options(settings, input, Utc::now(), false)
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
        self.preview_usage_at_with_options(settings, input, now, true)
    }

    fn preview_usage_at_with_options(
        &self,
        settings: &RuntimeSettings,
        input: VirtualUsageInput,
        now: DateTime<Utc>,
        allow_context_shrink_reset: bool,
    ) -> PendingVirtualUsage {
        let observed_total = input.observed_total_input_tokens.max(0);
        if !settings.virtual_cache_usage_enabled || observed_total == 0 {
            return PendingVirtualUsage {
                key: None,
                usage: AnthropicUsage::simple(observed_total, input.output_tokens),
                creation_ttl: input.creation_ttl,
                creation_tokens: 0,
                observed_total_input_tokens: observed_total,
                accounting_total_input_tokens: observed_total,
                reset_ledger: false,
            };
        }

        let key = LedgerKey {
            model: input.model.clone(),
            session_key: input.session_key.clone(),
        };

        let mut entry = self.ledgers.lock().get(&key).cloned().unwrap_or_default();
        expire_entry(&mut entry, now);
        let reset_ledger =
            allow_context_shrink_reset && should_reset_for_context_shrink(&entry, observed_total);
        if reset_ledger {
            entry = LedgerEntry::default();
        }

        let accounting_total = compute_accounting_total(&input, observed_total);
        let uncached = compute_uncached_tokens(settings, &input, accounting_total);
        cap_entry_cached_tokens(&mut entry, accounting_total.saturating_sub(uncached));
        let read_tokens = entry
            .cached_5m_tokens
            .saturating_add(entry.cached_1h_tokens);

        let creation_tokens = compute_creation_tokens(
            settings,
            &input,
            &entry,
            accounting_total,
            uncached,
            reset_ledger,
        );

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
            accounting_total_input_tokens: accounting_total,
            reset_ledger,
        }
    }

    fn commit_usage_at(&self, pending: PendingVirtualUsage, now: DateTime<Utc>) {
        let Some(key) = pending.key else {
            return;
        };

        let mut ledgers = self.ledgers.lock();
        let entry = ledgers.entry(key).or_default();
        if pending.reset_ledger {
            *entry = LedgerEntry::default();
        } else {
            expire_entry(entry, now);
        }

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
        cap_entry_cached_tokens(
            entry,
            pending
                .accounting_total_input_tokens
                .saturating_sub(pending.usage.input_tokens),
        );
        entry.last_observed_input_tokens = pending.observed_total_input_tokens;
        entry.last_accounting_input_tokens = pending.accounting_total_input_tokens;
        entry.turn_count = entry.turn_count.saturating_add(1);
    }
}

fn compute_accounting_total(input: &VirtualUsageInput, observed_total: i32) -> i32 {
    input
        .accounting_total_input_tokens
        .unwrap_or(observed_total)
        .max(observed_total)
        .max(0)
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
    reset_ledger: bool,
) -> i32 {
    if entry.turn_count == 0 {
        let cacheable_input = observed_total.saturating_sub(uncached);
        return if reset_ledger {
            cacheable_input
        } else {
            cacheable_input.max(settings.virtual_cache_warmup_tokens as i32)
        };
    }

    let delta = observed_total.saturating_sub(entry.last_accounting_input_tokens);
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

    // 两个桶都失效时，整个虚拟 cache 已不存在，对应 Anthropic 真实 cache 已过期，
    // 下次请求按"全量重建"计费。把 turn_count 和 last_observed_input_tokens 重置，
    // 让 compute_creation_tokens 走 warmup 分支，把整段 history 当成 creation 写入。
    let no_active_bucket =
        entry.cached_5m_expires_at.is_none() && entry.cached_1h_expires_at.is_none();
    if no_active_bucket {
        entry.turn_count = 0;
        entry.last_observed_input_tokens = 0;
        entry.last_accounting_input_tokens = 0;
    }
}

fn should_reset_for_context_shrink(entry: &LedgerEntry, observed_total: i32) -> bool {
    entry.turn_count > 0
        && entry.last_observed_input_tokens > 0
        && observed_total < entry.last_observed_input_tokens.saturating_mul(7) / 10
}

fn cap_entry_cached_tokens(entry: &mut LedgerEntry, max_total: i32) {
    let max_total = max_total.max(0);
    let total = entry
        .cached_5m_tokens
        .saturating_add(entry.cached_1h_tokens);
    if total <= max_total {
        return;
    }

    let mut excess = total.saturating_sub(max_total);
    let trim_5m = entry.cached_5m_tokens.min(excess);
    entry.cached_5m_tokens = entry.cached_5m_tokens.saturating_sub(trim_5m);
    excess = excess.saturating_sub(trim_5m);
    if entry.cached_5m_tokens == 0 {
        entry.cached_5m_expires_at = None;
    }

    if excess > 0 {
        let trim_1h = entry.cached_1h_tokens.min(excess);
        entry.cached_1h_tokens = entry.cached_1h_tokens.saturating_sub(trim_1h);
        if entry.cached_1h_tokens == 0 {
            entry.cached_1h_expires_at = None;
        }
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
        let trimmed = user_id.trim();
        if !trimmed.is_empty() {
            return metadata_user_id_cache_key(trimmed);
        }
    }

    if fallback_scope == "none" {
        format!("fallback:none:{model}:{}", uuid::Uuid::new_v4())
    } else {
        format!("fallback:model:{model}")
    }
}

fn metadata_user_id_cache_key(user_id: &str) -> String {
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(user_id) {
        let device_id = map
            .get("device_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let account_uuid = map
            .get("account_uuid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let user = map
            .get("user_id")
            .or_else(|| map.get("user"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let session_id = extract_session_id(user_id).unwrap_or_default();

        if !device_id.is_empty()
            || !account_uuid.is_empty()
            || !user.is_empty()
            || !session_id.is_empty()
        {
            return format!(
                "metadata:json:device={device_id}:account={account_uuid}:user={user}:session={session_id}"
            );
        }
    }

    format!("metadata:user:{user_id}")
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
    use crate::model::config::Config;

    fn settings() -> RuntimeSettings {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.same_account_retry_rules.clear();
        settings.virtual_cache_usage_enabled = true;
        settings.virtual_cache_default_ttl = "5m".to_string();
        settings.virtual_cache_uncached_input_tokens = 1;
        settings.virtual_cache_input_mode = "fixed".to_string();
        settings.virtual_cache_min_input_tokens = 8;
        settings.virtual_cache_max_input_tokens = 96;
        settings.virtual_cache_warmup_tokens = 18_000;
        settings.virtual_cache_min_creation_tokens = 128;
        settings.virtual_cache_max_creation_tokens = 1_200;
        settings.virtual_cache_creation_mode = "fixed".to_string();
        settings.virtual_cache_creation_jitter_ratio = 0.25;
        settings.virtual_cache_burst_every_turns = 7;
        settings.virtual_cache_burst_min_tokens = 1_500;
        settings.virtual_cache_burst_max_tokens = 3_000;
        settings.virtual_cache_fallback_scope = "model".to_string();
        settings
    }

    fn input(session: &str, observed: i32, ttl: CacheTtl) -> VirtualUsageInput {
        VirtualUsageInput {
            model: "claude-sonnet-4-5-20250929".to_string(),
            session_key: session.to_string(),
            observed_total_input_tokens: observed,
            accounting_total_input_tokens: None,
            estimated_uncached_input_tokens: None,
            output_tokens: 7,
            creation_ttl: ttl,
        }
    }

    #[test]
    fn same_session_cache_read_is_capped_by_observed_input() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let first = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        assert_eq!(first.input_tokens, 1);
        assert_eq!(first.cache_read_input_tokens, 0);
        assert_eq!(first.cache_creation_input_tokens, 18_000);

        let second =
            manager.build_usage(&settings, input("session-a", 1100, CacheTtl::FiveMinutes));
        assert_eq!(second.cache_read_input_tokens, 999);
        assert_eq!(second.cache_creation_input_tokens, 128);
    }

    #[test]
    fn sessions_and_models_are_isolated_but_credentials_share_client_cache() {
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

        let same_client_after_internal_credential_switch = manager.build_usage(
            &settings,
            VirtualUsageInput {
                ..input("session-a", 1100, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(
            same_client_after_internal_credential_switch.cache_read_input_tokens,
            999
        );
    }

    fn request_with_metadata_user_id(user_id: &str) -> MessagesRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-4-5-20250929",
            "max_tokens": 100,
            "metadata": {
                "user_id": user_id
            },
            "messages": [
                {
                    "role": "user",
                    "content": "hello"
                }
            ]
        }))
        .unwrap()
    }

    fn request_without_metadata() -> MessagesRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-4-5-20250929",
            "max_tokens": 100,
            "messages": [
                {
                    "role": "user",
                    "content": "hello"
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn metadata_json_cache_key_keeps_user_scope_and_session_scope() {
        let user_id = r#"{"session_id":"8bb5523b-ec7c-4540-a9ca-beb6d79f1552","account_uuid":"account-a","device_id":"device-a"}"#;
        let request = request_with_metadata_user_id(user_id);

        assert_eq!(
            session_key_for_request(&request, &request.model, "model"),
            "metadata:json:device=device-a:account=account-a:user=:session=8bb5523b-ec7c-4540-a9ca-beb6d79f1552"
        );
    }

    #[test]
    fn metadata_string_cache_key_keeps_full_user_id_not_only_session() {
        let user_id = "user_device-a_account__session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552";
        let request = request_with_metadata_user_id(user_id);

        assert_eq!(
            session_key_for_request(&request, &request.model, "model"),
            format!("metadata:user:{user_id}")
        );
    }

    #[test]
    fn missing_metadata_model_fallback_uses_model_ledger_key() {
        let first = request_without_metadata();
        let second = request_without_metadata();

        let first_key = session_key_for_request(&first, &first.model, "model");
        let second_key = session_key_for_request(&second, &second.model, "model");

        assert_eq!(first_key, "fallback:model:claude-sonnet-4-5-20250929");
        assert_eq!(second_key, first_key);
    }

    #[test]
    fn missing_metadata_none_fallback_uses_request_isolation() {
        let first = request_without_metadata();
        let second = request_without_metadata();

        let first_key = session_key_for_request(&first, &first.model, "none");
        let second_key = session_key_for_request(&second, &second.model, "none");

        assert!(first_key.starts_with("fallback:none:claude-sonnet-4-5-20250929:"));
        assert!(second_key.starts_with("fallback:none:claude-sonnet-4-5-20250929:"));
        assert_ne!(first_key, second_key);
    }

    #[test]
    fn missing_metadata_model_fallback_accumulates_virtual_cache() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let first = request_without_metadata();
        let second = request_without_metadata();

        let first_key = session_key_for_request(&first, &first.model, "model");
        let second_key = session_key_for_request(&second, &second.model, "model");

        let first_usage = manager.build_usage(
            &settings,
            VirtualUsageInput {
                session_key: first_key,
                ..input("unused", 30_000, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(first_usage.cache_read_input_tokens, 0);

        let second_usage = manager.build_usage(
            &settings,
            VirtualUsageInput {
                session_key: second_key,
                ..input("unused", 31_000, CacheTtl::FiveMinutes)
            },
        );
        assert!(
            second_usage.cache_read_input_tokens > 25_000,
            "model fallback should reuse the previous virtual cache ledger, actual = {}",
            second_usage.cache_read_input_tokens
        );
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
        assert_eq!(after_commit.usage().cache_read_input_tokens, 999);
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
    fn flat_usage_shape_omits_nested_cache_creation() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let usage = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        let json = usage.to_json_with_shape("flat");

        assert_eq!(json["cache_creation_input_tokens"], 18_000);
        assert!(json.get("cache_creation").is_none());
    }

    #[test]
    fn anthropic_usage_shape_includes_service_tier_and_geo() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let usage = manager.build_usage(&settings, input("session-a", 1000, CacheTtl::FiveMinutes));
        let json = usage.to_json_with_shape("anthropic");

        assert_eq!(json["service_tier"], "standard");
        assert_eq!(json["inference_geo"], "global");
        assert!(json.get("cache_creation").is_some());
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
        assert_eq!(one_hour.usage().cache_read_input_tokens, 999);
        manager.commit_usage_at(one_hour, now + Duration::seconds(1));

        let after_five_minutes = manager.preview_usage_at(
            &settings,
            input("session-a", 1200, CacheTtl::FiveMinutes),
            now + Duration::minutes(6),
        );
        assert_eq!(after_five_minutes.usage().cache_read_input_tokens, 128);
    }

    #[test]
    fn compressed_history_reduces_virtual_cache_read() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let _ = manager.build_usage(
            &settings,
            input("session-compressed", 200_000, CacheTtl::FiveMinutes),
        );

        let compressed = manager.build_usage(
            &settings,
            input("session-compressed", 5_000, CacheTtl::FiveMinutes),
        );
        assert_eq!(compressed.input_tokens, 1);
        assert_eq!(compressed.cache_read_input_tokens, 0);
        assert_eq!(compressed.cache_creation_input_tokens, 4_999);

        let next = manager.build_usage(
            &settings,
            input("session-compressed", 5_100, CacheTtl::FiveMinutes),
        );
        assert_eq!(next.cache_read_input_tokens, 4_999);
        assert_eq!(next.cache_creation_input_tokens, 128);
    }

    #[test]
    fn compressed_context_reset_keeps_virtual_accounting_total() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let _ = manager.build_usage(
            &settings,
            input(
                "session-compressed-accounting",
                200_000,
                CacheTtl::FiveMinutes,
            ),
        );

        let compressed = manager.build_usage(
            &settings,
            VirtualUsageInput {
                observed_total_input_tokens: 50_000,
                accounting_total_input_tokens: Some(200_000),
                ..input(
                    "session-compressed-accounting",
                    50_000,
                    CacheTtl::FiveMinutes,
                )
            },
        );
        assert_eq!(compressed.cache_read_input_tokens, 0);
        assert_eq!(compressed.cache_creation_input_tokens, 199_999);

        let next = manager.build_usage(
            &settings,
            VirtualUsageInput {
                observed_total_input_tokens: 51_000,
                accounting_total_input_tokens: Some(201_000),
                ..input(
                    "session-compressed-accounting",
                    51_000,
                    CacheTtl::FiveMinutes,
                )
            },
        );
        assert!(
            next.cache_read_input_tokens > 50_000,
            "虚拟 cache_read 不应被压缩后的上游 context usage 永久限制在 5w 附近，实际值 = {}",
            next.cache_read_input_tokens
        );
        assert_eq!(next.cache_creation_input_tokens, 1_000);
    }

    #[test]
    fn initial_preview_does_not_reset_on_mixed_local_and_context_usage_units() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();

        let first_final = manager.preview_usage(
            &settings,
            VirtualUsageInput {
                observed_total_input_tokens: 30_660,
                accounting_total_input_tokens: Some(30_660),
                estimated_uncached_input_tokens: Some(806),
                output_tokens: 80,
                ..input("session-real-user", 30_660, CacheTtl::FiveMinutes)
            },
        );
        manager.commit_usage(first_final);

        let ordinary_preview = manager.preview_usage(
            &settings,
            VirtualUsageInput {
                observed_total_input_tokens: 18_878,
                accounting_total_input_tokens: Some(18_878),
                estimated_uncached_input_tokens: Some(174),
                output_tokens: 1,
                ..input("session-real-user", 18_878, CacheTtl::FiveMinutes)
            },
        );
        assert_eq!(
            ordinary_preview.usage().cache_read_input_tokens,
            0,
            "普通最终口径 preview 遇到真实 context shrink 仍应 reset"
        );

        let initial_preview = manager.preview_usage_without_context_shrink_reset(
            &settings,
            VirtualUsageInput {
                observed_total_input_tokens: 18_878,
                accounting_total_input_tokens: Some(18_878),
                estimated_uncached_input_tokens: Some(174),
                output_tokens: 1,
                ..input("session-real-user", 18_878, CacheTtl::FiveMinutes)
            },
        );
        assert!(
            initial_preview.usage().cache_read_input_tokens > 0,
            "initial preview must keep prior final context ledger cache_read, actual = {}",
            initial_preview.usage().cache_read_input_tokens
        );
        assert!(
            initial_preview.usage().cache_creation_input_tokens < 18_000,
            "initial preview must not recreate the warmup-sized prompt cache, actual = {}",
            initial_preview.usage().cache_creation_input_tokens
        );
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

    #[test]
    fn buckets_all_expired_reset_entry_so_next_request_rebuilds_creation() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let now = Utc::now();

        // 建立 5m 桶
        let first = manager.preview_usage_at(
            &settings,
            input("session-rebuild", 200_000, CacheTtl::FiveMinutes),
            now,
        );
        manager.commit_usage_at(first, now);

        // 第二轮短增量
        let second = manager.preview_usage_at(
            &settings,
            input("session-rebuild", 200_050, CacheTtl::FiveMinutes),
            now + Duration::seconds(30),
        );
        let second_creation = second.usage().cache_creation_input_tokens;
        manager.commit_usage_at(second, now + Duration::seconds(30));
        assert!(
            second_creation <= settings.virtual_cache_max_creation_tokens as i32,
            "活跃中应走 delta 路径，creation 受 max 限制"
        );

        // 6 分钟后 5m 桶过期，且没有 1h 桶活动 → entry 整体失效
        let after_expiry = manager.preview_usage_at(
            &settings,
            input("session-rebuild", 200_100, CacheTtl::FiveMinutes),
            now + Duration::minutes(6),
        );
        let usage = after_expiry.usage();
        assert_eq!(
            usage.cache_read_input_tokens, 0,
            "桶都过期后 cache_read 必须从 0 开始"
        );
        // warmup 分支返回 max(observed - uncached, warmup_tokens)，
        // 对长 history（observed_total=200_100、uncached≈1）来说远大于 warmup_tokens，
        // 直接按整段 history 写入 → 模拟 Anthropic 真实"cache 过期后重写"的计费行为。
        assert!(
            usage.cache_creation_input_tokens >= 199_000,
            "桶整体过期后下次请求应走 warmup，按整段 history 计 creation，实际值 = {}",
            usage.cache_creation_input_tokens
        );
    }

    #[test]
    fn five_minute_expiry_with_one_hour_alive_keeps_entry_state() {
        let manager = VirtualCacheUsageManager::new();
        let settings = settings();
        let now = Utc::now();

        // 先在 5m 桶累一些
        let first = manager.preview_usage_at(
            &settings,
            input("session-mixed", 1000, CacheTtl::FiveMinutes),
            now,
        );
        manager.commit_usage_at(first, now);

        // 再在 1h 桶累一些
        let second = manager.preview_usage_at(
            &settings,
            input("session-mixed", 1100, CacheTtl::OneHour),
            now + Duration::seconds(10),
        );
        manager.commit_usage_at(second, now + Duration::seconds(10));

        // 第三轮，5m 桶累一些
        let third = manager.preview_usage_at(
            &settings,
            input("session-mixed", 1200, CacheTtl::FiveMinutes),
            now + Duration::seconds(20),
        );
        manager.commit_usage_at(third, now + Duration::seconds(20));

        // 6 分钟后 5m 过期，1h 还活着 → entry 不应被重置
        let after = manager.preview_usage_at(
            &settings,
            input("session-mixed", 1300, CacheTtl::FiveMinutes),
            now + Duration::minutes(6),
        );
        let usage = after.usage();
        assert!(
            usage.cache_read_input_tokens > 0,
            "1h 桶还在，cache_read 必须非零（继承 1h 累积）"
        );
        // 仍走 delta 路径，creation 受 max 限制（不是 warmup 的大值）
        assert!(
            usage.cache_creation_input_tokens <= settings.virtual_cache_max_creation_tokens as i32,
            "5m 过期但 1h 还在时不能走 warmup 分支，creation 受 max_creation_tokens 限制"
        );
    }
}
