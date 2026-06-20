//! Admin API 类型定义

use serde::{Deserialize, Serialize};

use crate::anthropic::VirtualCacheReuseSnapshot;
use crate::common::api_keys::ApiKeyRecord;
use crate::kiro::dynamic_proxy::{DynamicProxyBindingView, DynamicProxySummary};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model_cooldown::ModelCooldownSnapshot;
use crate::kiro::settings::RuntimeSettings;
use crate::metrics::RuntimeMetricsSnapshot;

// ============ 凭据状态 ============

/// 所有凭据状态响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatusResponse {
    /// 凭据总数
    pub total: usize,
    /// 可用凭据数量（未禁用）
    pub available: usize,
    /// 当前活跃凭据 ID
    pub current_id: u64,
    /// 各凭据状态列表
    pub credentials: Vec<CredentialStatusItem>,
}

/// 单个凭据的状态信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatusItem {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级（数字越小优先级越高）
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// 是否为当前活跃凭据
    pub is_current: bool,
    /// Token 过期时间（RFC3339 格式）
    pub expires_at: Option<String>,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 登录 Provider（Enterprise / BuilderId / Google / Github 等）
    pub provider: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// refreshToken 的 SHA-256 哈希（仅 OAuth 凭据，用于前端去重）
    pub refresh_token_hash: Option<String>,
    /// kiroApiKey 的 SHA-256 哈希（仅 API Key 凭据，用于前端去重）
    pub api_key_hash: Option<String>,
    /// kiroApiKey 的脱敏展示（仅 API Key 凭据，用于前端显示）
    pub masked_api_key: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// 订阅标题（本地缓存）
    pub subscription_title: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 是否配置了凭据级代理
    pub has_proxy: bool,
    /// 代理 URL（用于前端展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 禁用原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// 端点名称（决定该凭据走哪套 Kiro API，已回退到默认端点）
    pub endpoint: String,
    /// 是否允许该账号在本地额度快照达到上限后继续调度
    pub allow_overage: bool,
    /// 透支后的调度权重（1-10）
    pub overage_weight: u32,
    /// 是否因上游 OVERAGE 拒绝而停止透支
    pub overage_stopped: bool,
    /// 本地缓存的当前额度使用量
    pub usage_current: f64,
    /// 本地缓存的额度上限
    pub usage_limit: f64,
    /// 本地缓存的额度使用比例
    pub usage_percentage: f64,
    /// 本地缓存是否显示已达到额度上限
    pub is_over_usage_limit: bool,
    /// 当前正在处理的请求数
    pub in_flight: u32,
    /// 单账号最大并发数
    pub max_concurrent: usize,
    /// 单账号并发覆盖值
    pub max_concurrent_override: Option<usize>,
    /// 单账号 RPM 覆盖值
    pub rpm_override: Option<u32>,
    /// Turbo 模式（off/race）
    pub turbo_mode: String,
    /// Turbo 并发倍数
    pub turbo_fanout: usize,
    /// 当前生效 RPM
    pub effective_rpm: u32,
    /// 是否使用全局默认策略
    pub uses_default_policy: bool,
    /// 冷却截止时间（RFC3339）
    pub cooldown_until: Option<String>,
    /// 是否正在冷却
    pub is_cooling_down: bool,
    /// 当前是否可被调度
    pub available_for_dispatch: bool,
    /// 绑定到该凭据的活跃会话数
    pub session_affinity_bindings: usize,
    /// 动态代理绑定状态
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_proxy: Option<DynamicProxyBindingView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialTestRequest {
    pub model: String,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialTestResponse {
    pub credential_id: u64,
    pub model: String,
    pub prompt: String,
    pub response_text: String,
    pub status: u16,
    pub latency_ms: u64,
    pub endpoint: String,
    pub api_region: String,
}

// ============ 外部 API 密钥 ============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeysResponse {
    pub keys: Vec<ApiKeyItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyItem {
    pub id: u64,
    pub name: String,
    pub key: String,
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

impl From<ApiKeyRecord> for ApiKeyItem {
    fn from(value: ApiKeyRecord) -> Self {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiKeyRequest {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub disabled: Option<bool>,
}

// ============ 运行时状态 ============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusResponse {
    pub default_endpoint: String,
    pub endpoints: Vec<EndpointOption>,
    pub global_in_flight: usize,
    pub global_max_concurrent: usize,
    pub per_account_default_max_concurrent: usize,
    pub global_rpm: u32,
    pub per_account_default_rpm: u32,
    pub queue_depth: usize,
    pub queue_max_size: usize,
    pub queue_timeout_ms: u64,
    pub rate_limit_cooldown_ms: u64,
    pub transient_cooldown_ms: u64,
    pub max_retry_accounts: usize,
    pub allow_over_usage: bool,
    pub model_capacity_cooldown_ms: u64,
    pub same_account_retry_rules: Vec<crate::kiro::settings::SameAccountRetryRule>,
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
    pub virtual_cache_reuse: VirtualCacheReuseSnapshot,
    pub total_credentials: usize,
    pub available_credentials: usize,
    pub dispatch_available_credentials: usize,
    pub cooling_down_credentials: usize,
    pub session_affinity_bindings: usize,
    pub request_metrics: RuntimeMetricsSnapshot,
    pub model_cooldowns: Vec<ModelCooldownSnapshot>,
    pub dynamic_proxy: DynamicProxySummary,
    pub credentials: Vec<RuntimeCredentialStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointOption {
    pub name: String,
    pub label: String,
    pub api_url: String,
    pub is_default: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointConfigResponse {
    pub default_endpoint: String,
    pub endpoints: Vec<EndpointOption>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointLatencyResponse {
    pub endpoint: String,
    pub label: String,
    pub api_url: String,
    pub network_ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCredentialStatus {
    pub id: u64,
    pub in_flight: u32,
    pub max_concurrent: usize,
    pub max_concurrent_override: Option<usize>,
    pub rpm_override: Option<u32>,
    pub turbo_mode: String,
    pub turbo_fanout: usize,
    pub effective_rpm: u32,
    pub uses_default_policy: bool,
    pub cooldown_until: Option<String>,
    pub is_cooling_down: bool,
    pub available_for_dispatch: bool,
    pub session_affinity_bindings: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_proxy: Option<DynamicProxyBindingView>,
}

// ============ 操作请求 ============

/// 启用/禁用凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDisabledRequest {
    /// 是否禁用
    pub disabled: bool,
}

/// 修改优先级请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPriorityRequest {
    /// 新优先级值
    pub priority: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCredentialPolicyRequest {
    pub max_concurrent_override: Option<usize>,
    pub rpm_override: Option<u32>,
    #[serde(default = "default_turbo_mode")]
    pub turbo_mode: String,
    #[serde(default = "default_turbo_fanout")]
    pub turbo_fanout: usize,
    pub allow_overage: bool,
    pub overage_weight: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchCredentialPolicyRequest {
    pub ids: Vec<u64>,
    pub max_concurrent_override: Option<usize>,
    pub rpm_override: Option<u32>,
    #[serde(default = "default_turbo_mode")]
    pub turbo_mode: String,
    #[serde(default = "default_turbo_fanout")]
    pub turbo_fanout: usize,
    pub allow_overage: bool,
    pub overage_weight: u32,
}

fn default_turbo_mode() -> String {
    "off".to_string()
}

fn default_turbo_fanout() -> usize {
    1
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchCredentialIdsRequest {
    pub ids: Vec<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyBindingsResponse {
    pub bindings: Vec<DynamicProxyBindingView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyActionResponse {
    pub success: bool,
    pub binding: Option<DynamicProxyBindingView>,
    pub attempts: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyBatchActionResponse {
    pub success: bool,
    pub requested: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

pub type RuntimeSettingsResponse = RuntimeSettings;
pub type SetRuntimeSettingsRequest = RuntimeSettings;

/// 添加凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialRequest {
    /// 刷新令牌（OAuth 凭据必填，API Key 凭据不需要）
    pub refresh_token: Option<String>,

    /// 访问令牌（导入完整凭据时可选；OAuth 凭据仍会优先刷新）
    pub access_token: Option<String>,

    /// Token 过期时间（RFC3339，导入完整凭据时可选）
    pub expires_at: Option<String>,

    /// Profile ARN（导入完整凭据时可选）
    pub profile_arn: Option<String>,

    /// 认证方式（可选，默认 social）
    #[serde(default = "default_auth_method")]
    pub auth_method: String,

    /// 登录 Provider（可选，用于区分 Enterprise IdC 和 BuilderId）
    pub provider: Option<String>,

    /// OIDC Client ID（IdC 认证需要）
    pub client_id: Option<String>,

    /// OIDC Client Secret（IdC 认证需要）
    pub client_secret: Option<String>,

    /// 优先级（可选，默认 0）
    #[serde(default)]
    pub priority: u32,

    /// 凭据级 Region 配置（用于 OIDC token 刷新）
    /// 未配置时回退到 config.json 的全局 region
    pub region: Option<String>,

    /// 凭据级 Auth Region（用于 Token 刷新）
    pub auth_region: Option<String>,

    /// 凭据级 API Region（用于 API 请求）
    pub api_region: Option<String>,

    /// 凭据级 Machine ID（可选，64 位字符串）
    /// 未配置时回退到 config.json 的 machineId
    pub machine_id: Option<String>,

    /// 用户邮箱（可选，用于前端显示）
    pub email: Option<String>,

    /// 凭据级代理 URL（可选，特殊值 "direct" 表示不使用代理）
    pub proxy_url: Option<String>,

    /// 凭据级代理认证用户名（可选）
    pub proxy_username: Option<String>,

    /// 凭据级代理认证密码（可选）
    pub proxy_password: Option<String>,

    /// Kiro API Key（API Key 凭据必填，格式: ksk_xxxxxxxx）
    /// 设置后直接作为 Bearer Token 使用，无需 refreshToken
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiro_api_key: Option<String>,

    /// 端点名称（可选，未配置时使用 config.defaultEndpoint）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,

    /// 订阅标题（导入缓存值，后续余额查询会刷新）
    pub subscription_title: Option<String>,

    /// 本地额度满后是否仍允许调度该账号
    #[serde(default)]
    pub allow_overage: bool,

    /// 透支后的调度权重（1-10；0 表示默认）
    #[serde(default)]
    pub overage_weight: u32,

    /// 导入时携带的本地额度快照
    pub usage_current: Option<f64>,

    /// 导入时携带的本地额度上限
    pub usage_limit: Option<f64>,

    /// 是否已因上游 OVERAGE 拒绝而停止透支
    #[serde(default)]
    pub overage_stopped: bool,
}

fn default_auth_method() -> String {
    "social".to_string()
}

/// 添加凭据成功响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialResponse {
    pub success: bool,
    pub message: String,
    /// 新添加的凭据 ID
    pub credential_id: u64,
    /// 用户邮箱（如果获取成功）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

// ============ 凭据导出 ============

/// 批量导出凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCredentialsRequest {
    /// 要导出的凭据 ID 列表
    pub ids: Vec<u64>,
}

/// 批量导出凭据响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCredentialsResponse {
    /// 导出的凭据数量
    pub count: usize,
    /// 明文凭据列表，仅由受 Admin API Key 保护的导出接口返回
    pub credentials: Vec<KiroCredentials>,
}

// ============ 余额查询 ============

/// 余额查询响应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// 凭据 ID
    pub id: u64,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 当前使用量
    pub current_usage: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 剩余额度
    pub remaining: f64,
    /// 使用百分比
    pub usage_percentage: f64,
    /// 下次重置时间（Unix 时间戳）
    pub next_reset_at: Option<f64>,
}

// ============ 负载均衡配置 ============

/// 负载均衡模式响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancingModeResponse {
    /// 当前模式（"priority" 或 "balanced"）
    pub mode: String,
}

/// 设置负载均衡模式请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLoadBalancingModeRequest {
    /// 模式（"priority" 或 "balanced"）
    pub mode: String,
}

// ============ 通用响应 ============

/// 操作成功响应
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
    pub message: String,
}

impl SuccessResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

/// 错误响应
#[derive(Debug, Serialize)]
pub struct AdminErrorResponse {
    pub error: AdminError,
}

#[derive(Debug, Serialize)]
pub struct AdminError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl AdminErrorResponse {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: AdminError {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid or missing admin API key")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new("api_error", message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new("internal_error", message)
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchCredentialPolicyRequest, SetCredentialPolicyRequest};

    #[test]
    fn policy_request_defaults_turbo_fields() {
        let single: SetCredentialPolicyRequest = serde_json::from_str(
            r#"{
                "maxConcurrentOverride": null,
                "rpmOverride": null,
                "allowOverage": false,
                "overageWeight": 1
            }"#,
        )
        .unwrap();
        assert_eq!(single.turbo_mode, "off");
        assert_eq!(single.turbo_fanout, 1);

        let batch: BatchCredentialPolicyRequest = serde_json::from_str(
            r#"{
                "ids": [1, 2],
                "maxConcurrentOverride": null,
                "rpmOverride": null,
                "allowOverage": false,
                "overageWeight": 1
            }"#,
        )
        .unwrap();
        assert_eq!(batch.turbo_mode, "off");
        assert_eq!(batch.turbo_fanout, 1);
    }
}
