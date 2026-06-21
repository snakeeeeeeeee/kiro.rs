use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::kiro::settings::SameAccountRetryRule;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsBackend {
    Rustls,
    NativeTls,
}

impl Default for TlsBackend {
    fn default() -> Self {
        Self::Rustls
    }
}

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    /// Auth Region（用于 Token 刷新），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,

    /// API Region（用于 API 请求），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    /// 是否打印上游请求摘要诊断日志（不包含 prompt/token 明文）
    #[serde(default)]
    pub request_diagnostics_enabled: bool,

    #[serde(default = "default_tls_backend")]
    pub tls_backend: TlsBackend,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// 负载均衡模式（"priority" 或 "balanced"）
    #[serde(default = "default_load_balancing_mode")]
    pub load_balancing_mode: String,

    /// 全局最大并发请求数
    #[serde(default = "default_global_max_concurrent")]
    pub global_max_concurrent: usize,

    /// 全局最大并发配置上限
    #[serde(default = "default_global_max_concurrent_limit")]
    pub global_max_concurrent_limit: usize,

    /// 单个凭据最大并发请求数
    #[serde(default = "default_per_account_max_concurrent")]
    pub per_account_max_concurrent: usize,

    /// 全局等待队列最大长度
    #[serde(default = "default_queue_max_size")]
    pub queue_max_size: usize,

    /// 等待队列超时时间（毫秒）
    #[serde(default = "default_queue_timeout_ms")]
    pub queue_timeout_ms: u64,

    /// 单凭据每分钟请求数限制，0 表示不限制
    #[serde(default)]
    pub per_account_rpm: u32,

    /// 全局每分钟请求数限制，0 表示不限制
    #[serde(default)]
    pub global_rpm: u32,

    /// 上游限流后的账号冷却时间（毫秒）
    #[serde(default = "default_rate_limit_cooldown_ms")]
    pub rate_limit_cooldown_ms: u64,

    /// 上游瞬态错误后的账号冷却时间（毫秒）
    #[serde(default = "default_transient_cooldown_ms")]
    pub transient_cooldown_ms: u64,

    /// 单次请求最多尝试的不同账号数
    #[serde(default = "default_max_retry_accounts")]
    pub max_retry_accounts: usize,

    /// 是否允许已达到本地额度快照上限的账号继续参与调度
    #[serde(default)]
    pub allow_over_usage: bool,

    /// 模型容量不足后的模型级冷却时间（毫秒）
    #[serde(default = "default_model_capacity_cooldown_ms")]
    pub model_capacity_cooldown_ms: u64,

    /// 单账号原地重试规则。匹配后先同号重试，耗尽后才进入账号冷却/换号逻辑。
    #[serde(default = "default_same_account_retry_rules")]
    pub same_account_retry_rules: Vec<SameAccountRetryRule>,

    /// 是否启用后台 Token 自动刷新
    #[serde(default = "default_token_auto_refresh_enabled")]
    pub token_auto_refresh_enabled: bool,

    /// 后台 Token 自动刷新扫描间隔（秒）
    #[serde(default = "default_token_auto_refresh_interval_secs")]
    pub token_auto_refresh_interval_secs: u64,

    /// Token 距离过期多少秒内触发后台刷新
    #[serde(default = "default_token_auto_refresh_window_secs")]
    pub token_auto_refresh_window_secs: u64,

    /// 是否启用会话到账号的软亲和绑定
    #[serde(default = "default_session_affinity_enabled")]
    pub session_affinity_enabled: bool,

    /// 会话亲和绑定 TTL（秒）
    #[serde(default = "default_session_affinity_ttl_secs")]
    pub session_affinity_ttl_secs: u64,

    /// Opus 4.7 plain 稳定模式："off"、"adaptive_low" 或 "adaptive_high"
    #[serde(default = "default_opus47_plain_stabilization_mode")]
    pub opus47_plain_stabilization_mode: String,

    /// Opus 4.7 ANTML 探针兼容模式："off" 或 "clarify"
    #[serde(default = "default_opus47_antml_probe_compat")]
    pub opus47_antml_probe_compat: String,

    /// Opus 4.7 clean probe 模式："off" 或 "clean"
    #[serde(default = "default_opus47_clean_probe_mode")]
    pub opus47_clean_probe_mode: String,

    /// Opus 4.7 检测 profile："normal"、"cc_max_like" 或 "clean_probe_debug"
    #[serde(default = "default_opus47_detection_profile")]
    pub opus47_detection_profile: String,

    /// Opus 4.7 signed-thinking 保留实验："off"、"diagnose"、"cache_only" 或 "history_experiment"
    #[serde(default = "default_opus47_signed_thinking_preservation")]
    pub opus47_signed_thinking_preservation: String,

    /// Opus 4.7 短请求 thinking 标签实验："off" 或 "adaptive_high"
    #[serde(default = "default_opus47_short_thinking_experiment")]
    pub opus47_short_thinking_experiment: String,

    /// 是否启用 Opus 4.7 响应形态诊断日志
    #[serde(default = "default_opus47_diagnostics_enabled")]
    pub opus47_diagnostics_enabled: bool,

    /// 是否启用 Opus 4.7 原始请求/响应调试日志（会记录正文，默认关闭）
    #[serde(default)]
    pub opus47_raw_debug_enabled: bool,

    /// Opus 4.7 原始调试日志单字段最大字符数
    #[serde(default = "default_opus47_raw_debug_max_chars")]
    pub opus47_raw_debug_max_chars: usize,

    /// Opus 4.6 检测 profile："normal" 或 "cc_max_like"
    #[serde(default = "default_opus46_detection_profile")]
    pub opus46_detection_profile: String,

    /// Opus 4.6 ANTML 探针兼容模式："off" 或 "clarify"
    #[serde(default = "default_opus46_antml_probe_compat")]
    pub opus46_antml_probe_compat: String,

    /// 是否启用 Opus 4.6 响应形态诊断日志
    #[serde(default = "default_opus46_diagnostics_enabled")]
    pub opus46_diagnostics_enabled: bool,

    /// 是否启用 Opus 4.6 原始请求/响应调试日志（会记录正文，默认关闭）
    #[serde(default)]
    pub opus46_raw_debug_enabled: bool,

    /// Opus 4.6 原始调试日志单字段最大字符数
    #[serde(default = "default_opus46_raw_debug_max_chars")]
    pub opus46_raw_debug_max_chars: usize,

    /// Sonnet 4.6 检测 profile："normal" 或 "cc_max_like"
    #[serde(default = "default_sonnet46_detection_profile")]
    pub sonnet46_detection_profile: String,

    /// Sonnet 4.6 ANTML 探针兼容模式："off" 或 "clarify"
    #[serde(default = "default_sonnet46_antml_probe_compat")]
    pub sonnet46_antml_probe_compat: String,

    /// 是否启用 Sonnet 4.6 响应形态诊断日志
    #[serde(default = "default_sonnet46_diagnostics_enabled")]
    pub sonnet46_diagnostics_enabled: bool,

    /// 是否启用 Sonnet 4.6 原始请求/响应调试日志（会记录正文，默认关闭）
    #[serde(default)]
    pub sonnet46_raw_debug_enabled: bool,

    /// Sonnet 4.6 原始调试日志单字段最大字符数
    #[serde(default = "default_sonnet46_raw_debug_max_chars")]
    pub sonnet46_raw_debug_max_chars: usize,

    /// 是否保存客户端请求、上游 prompt 和上游原始响应到文件（默认关闭）
    #[serde(default)]
    pub prompt_dump_enabled: bool,

    /// Prompt dump 目录
    #[serde(default = "default_prompt_dump_dir")]
    pub prompt_dump_dir: String,

    /// Prompt dump 单文件最大字节数
    #[serde(default = "default_prompt_dump_max_bytes")]
    pub prompt_dump_max_bytes: usize,

    /// Prompt dump 模型 allowlist，逗号分隔
    #[serde(default = "default_prompt_dump_models")]
    pub prompt_dump_models: String,

    /// 是否启用 Kiro 上游请求历史消息裁剪
    #[serde(default)]
    pub message_pruning_enabled: bool,

    /// Kiro 上游请求体字节上限，超过后才触发裁剪
    #[serde(default = "default_message_pruning_max_request_bytes")]
    pub message_pruning_max_request_bytes: usize,

    /// 裁剪时最少保留的历史消息数量
    #[serde(default = "default_message_pruning_keep_recent_messages")]
    pub message_pruning_keep_recent_messages: usize,

    /// 单条历史消息超过该字节数时允许进入文本截断
    #[serde(default = "default_message_pruning_max_history_entry_bytes")]
    pub message_pruning_max_history_entry_bytes: usize,

    /// 文本字段截断后的最大字节数
    #[serde(default = "default_message_pruning_max_truncated_content_bytes")]
    pub message_pruning_max_truncated_content_bytes: usize,

    /// 兼容 usage 字段形态："anthropic" 或 "flat"
    #[serde(default = "default_compat_usage_shape")]
    pub compat_usage_shape: String,

    /// 兼容 thinking 模型响应："native" 或 "plain_text"
    #[serde(default = "default_compat_thinking_model")]
    pub compat_thinking_model: String,

    /// 兼容 /v1/models 输出形态："anthropic" 或 "aggregator"
    #[serde(default = "default_compat_models_shape")]
    pub compat_models_shape: String,

    /// 是否启用虚拟缓存 usage 字段（用于下游网关计费展示）
    #[serde(default = "default_virtual_cache_usage_enabled")]
    pub virtual_cache_usage_enabled: bool,

    /// 未显式 cache_control 时的默认虚拟缓存 TTL："5m" 或 "1h"
    #[serde(default = "default_virtual_cache_default_ttl")]
    pub virtual_cache_default_ttl: String,

    /// 虚拟拆账中保留为普通输入的 tokens，默认 1
    #[serde(default = "default_virtual_cache_uncached_input_tokens")]
    pub virtual_cache_uncached_input_tokens: u32,

    /// 虚拟普通输入计算模式："fixed" 或 "estimated_user_delta"
    #[serde(default = "default_virtual_cache_input_mode")]
    pub virtual_cache_input_mode: String,

    /// 动态普通输入最小 tokens
    #[serde(default = "default_virtual_cache_min_input_tokens")]
    pub virtual_cache_min_input_tokens: u32,

    /// 动态普通输入最大 tokens
    #[serde(default = "default_virtual_cache_max_input_tokens")]
    pub virtual_cache_max_input_tokens: u32,

    /// 首轮虚拟缓存创建下限
    #[serde(default = "default_virtual_cache_warmup_tokens")]
    pub virtual_cache_warmup_tokens: u32,

    /// 后续轮次虚拟缓存创建下限
    #[serde(default = "default_virtual_cache_min_creation_tokens")]
    pub virtual_cache_min_creation_tokens: u32,

    /// 后续轮次虚拟缓存创建上限
    #[serde(default = "default_virtual_cache_max_creation_tokens")]
    pub virtual_cache_max_creation_tokens: u32,

    /// 虚拟缓存创建计算模式："fixed" 或 "dynamic"
    #[serde(default = "default_virtual_cache_creation_mode")]
    pub virtual_cache_creation_mode: String,

    /// 动态缓存创建抖动比例，0.25 表示 +/-25%
    #[serde(default = "default_virtual_cache_creation_jitter_ratio")]
    pub virtual_cache_creation_jitter_ratio: f64,

    /// 每多少轮追加一次较大的动态缓存创建，0 表示关闭
    #[serde(default = "default_virtual_cache_burst_every_turns")]
    pub virtual_cache_burst_every_turns: u32,

    /// 动态突增缓存创建最小 tokens
    #[serde(default = "default_virtual_cache_burst_min_tokens")]
    pub virtual_cache_burst_min_tokens: u32,

    /// 动态突增缓存创建最大 tokens
    #[serde(default = "default_virtual_cache_burst_max_tokens")]
    pub virtual_cache_burst_max_tokens: u32,

    /// 无 metadata 时的 fallback 策略："model" 为兼容旧配置的单请求隔离，"none" 为不累计
    #[serde(default = "default_virtual_cache_fallback_scope")]
    pub virtual_cache_fallback_scope: String,

    /// Haiku 模型是否跳过虚拟缓存读写，直接按普通输入上报 usage
    #[serde(default = "default_virtual_cache_haiku_input_only_enabled")]
    pub virtual_cache_haiku_input_only_enabled: bool,

    /// 全局目标缓存复用率，0 表示关闭；范围 0..1
    #[serde(default = "default_target_cache_reuse_ratio")]
    pub target_cache_reuse_ratio: f64,

    /// 上下文压缩判定阈值，0 表示关闭；范围 0..1
    #[serde(default = "default_virtual_cache_context_shrink_reset_ratio")]
    pub virtual_cache_context_shrink_reset_ratio: f64,

    /// 是否启用动态账号代理绑定
    #[serde(default)]
    pub dynamic_proxy_enabled: bool,

    /// 动态代理供应商标识，默认 novproxy
    #[serde(default = "default_dynamic_proxy_provider")]
    pub dynamic_proxy_provider: String,

    /// 动态代理协议："http" 或 "socks5"
    #[serde(default = "default_dynamic_proxy_protocol")]
    pub dynamic_proxy_protocol: String,

    /// 动态代理 host
    #[serde(default = "default_dynamic_proxy_host")]
    pub dynamic_proxy_host: String,

    /// 动态代理端口
    #[serde(default = "default_dynamic_proxy_port")]
    pub dynamic_proxy_port: u16,

    /// 动态代理用户名模板，支持 {region}/{state}/{sid}/{ttl}
    #[serde(default = "default_dynamic_proxy_username_template")]
    pub dynamic_proxy_username_template: String,

    /// 动态代理密码
    #[serde(default)]
    pub dynamic_proxy_password: String,

    /// 动态代理地区
    #[serde(default = "default_dynamic_proxy_region")]
    pub dynamic_proxy_region: String,

    /// 动态代理州/省
    #[serde(default = "default_dynamic_proxy_state")]
    pub dynamic_proxy_state: String,

    /// 动态代理绑定 TTL（分钟）
    #[serde(default = "default_dynamic_proxy_ttl_minutes")]
    pub dynamic_proxy_ttl_minutes: u32,

    /// 距离过期多少毫秒内自动换绑
    #[serde(default = "default_dynamic_proxy_renew_before_ms")]
    pub dynamic_proxy_renew_before_ms: u64,

    /// 动态代理出口验证 URL
    #[serde(default = "default_dynamic_proxy_verify_url")]
    pub dynamic_proxy_verify_url: String,

    /// 动态代理绑定最大重试次数
    #[serde(default = "default_dynamic_proxy_max_bind_retries")]
    pub dynamic_proxy_max_bind_retries: u32,

    /// 是否自动为新账号绑定动态代理
    #[serde(default)]
    pub dynamic_proxy_auto_bind_new_accounts: bool,

    /// 动态代理后台维护间隔（毫秒）
    #[serde(default = "default_dynamic_proxy_worker_interval_ms")]
    pub dynamic_proxy_worker_interval_ms: u64,

    /// 动态代理后台每轮处理数量
    #[serde(default = "default_dynamic_proxy_worker_batch_size")]
    pub dynamic_proxy_worker_batch_size: usize,

    /// 动态代理后台并发数
    #[serde(default = "default_dynamic_proxy_worker_concurrency")]
    pub dynamic_proxy_worker_concurrency: usize,

    /// 优雅关闭等待正在处理请求的时间（秒）
    #[serde(default = "default_shutdown_drain_timeout_secs")]
    pub shutdown_drain_timeout_secs: u64,

    /// 是否开启非流式响应的 thinking 块提取（默认 true）
    ///
    /// 启用后，非流式响应中的 `<thinking>...</thinking>` 标签会被解析为
    /// 独立的 `{"type": "thinking", ...}` 内容块,与流式响应行为一致。
    #[serde(default = "default_extract_thinking")]
    pub extract_thinking: bool,

    /// 默认端点名称（凭据未显式指定 endpoint 时使用，默认 "ide"）
    /// 支持：ide / codewhisperer / amazonq
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 端点特定的配置
    ///
    /// 键为端点名（如 "ide" / "codewhisperer" / "amazonq"），值为该端点自由定义的参数对象。
    /// 未在此表出现的端点沿用实现内置默认值。
    #[serde(default)]
    pub endpoints: HashMap<String, serde_json::Value>,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.12.155".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.22.0".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

fn default_tls_backend() -> TlsBackend {
    TlsBackend::Rustls
}

fn default_load_balancing_mode() -> String {
    "priority".to_string()
}

fn default_global_max_concurrent() -> usize {
    32
}

fn default_global_max_concurrent_limit() -> usize {
    512
}

fn default_per_account_max_concurrent() -> usize {
    3
}

fn default_queue_max_size() -> usize {
    128
}

fn default_queue_timeout_ms() -> u64 {
    30_000
}

fn default_rate_limit_cooldown_ms() -> u64 {
    60_000
}

fn default_transient_cooldown_ms() -> u64 {
    10_000
}

fn default_max_retry_accounts() -> usize {
    3
}

fn default_model_capacity_cooldown_ms() -> u64 {
    10_000
}

fn default_same_account_retry_rules() -> Vec<SameAccountRetryRule> {
    vec![SameAccountRetryRule {
        enabled: true,
        status: "429".to_string(),
        reason: Some("INSUFFICIENT_MODEL_CAPACITY".to_string()),
        attempts: 2,
        delay_ms: 1_500,
        respect_retry_after: true,
    }]
}

fn default_token_auto_refresh_enabled() -> bool {
    true
}

fn default_token_auto_refresh_interval_secs() -> u64 {
    300
}

fn default_token_auto_refresh_window_secs() -> u64 {
    1_800
}

fn default_session_affinity_enabled() -> bool {
    true
}

fn default_session_affinity_ttl_secs() -> u64 {
    3_600
}

fn default_opus47_plain_stabilization_mode() -> String {
    "off".to_string()
}

fn default_opus47_antml_probe_compat() -> String {
    "off".to_string()
}

fn default_opus47_clean_probe_mode() -> String {
    "off".to_string()
}

fn default_opus47_detection_profile() -> String {
    "normal".to_string()
}

fn default_opus47_signed_thinking_preservation() -> String {
    "off".to_string()
}

fn default_opus47_short_thinking_experiment() -> String {
    "off".to_string()
}

fn default_opus47_diagnostics_enabled() -> bool {
    true
}

fn default_opus47_raw_debug_max_chars() -> usize {
    20_000
}

fn default_opus46_detection_profile() -> String {
    "normal".to_string()
}

fn default_opus46_antml_probe_compat() -> String {
    "off".to_string()
}

fn default_opus46_diagnostics_enabled() -> bool {
    true
}

fn default_opus46_raw_debug_max_chars() -> usize {
    20_000
}

fn default_sonnet46_detection_profile() -> String {
    "normal".to_string()
}

fn default_sonnet46_antml_probe_compat() -> String {
    "off".to_string()
}

fn default_sonnet46_diagnostics_enabled() -> bool {
    true
}

fn default_sonnet46_raw_debug_max_chars() -> usize {
    20_000
}

fn default_prompt_dump_dir() -> String {
    "/app/config/prompt-dumps".to_string()
}

fn default_prompt_dump_max_bytes() -> usize {
    2_000_000
}

fn default_prompt_dump_models() -> String {
    "claude-opus-4-6,claude-opus-4-7,claude-opus-4-8,claude-sonnet-4-6".to_string()
}

fn default_message_pruning_max_request_bytes() -> usize {
    615 * 1024
}

fn default_message_pruning_keep_recent_messages() -> usize {
    2
}

fn default_message_pruning_max_history_entry_bytes() -> usize {
    300_000
}

fn default_message_pruning_max_truncated_content_bytes() -> usize {
    50_000
}

fn default_compat_usage_shape() -> String {
    "anthropic".to_string()
}

fn default_compat_thinking_model() -> String {
    "native".to_string()
}

fn default_compat_models_shape() -> String {
    "anthropic".to_string()
}

fn default_virtual_cache_usage_enabled() -> bool {
    true
}

fn default_virtual_cache_default_ttl() -> String {
    "5m".to_string()
}

fn default_virtual_cache_uncached_input_tokens() -> u32 {
    1
}

fn default_virtual_cache_input_mode() -> String {
    "fixed".to_string()
}

fn default_virtual_cache_min_input_tokens() -> u32 {
    8
}

fn default_virtual_cache_max_input_tokens() -> u32 {
    96
}

fn default_virtual_cache_warmup_tokens() -> u32 {
    18_000
}

fn default_virtual_cache_min_creation_tokens() -> u32 {
    128
}

fn default_virtual_cache_max_creation_tokens() -> u32 {
    1_200
}

fn default_virtual_cache_creation_mode() -> String {
    "fixed".to_string()
}

fn default_virtual_cache_creation_jitter_ratio() -> f64 {
    0.25
}

fn default_virtual_cache_burst_every_turns() -> u32 {
    7
}

fn default_virtual_cache_burst_min_tokens() -> u32 {
    1_500
}

fn default_virtual_cache_burst_max_tokens() -> u32 {
    3_000
}

fn default_virtual_cache_fallback_scope() -> String {
    "none".to_string()
}

fn default_virtual_cache_haiku_input_only_enabled() -> bool {
    false
}

fn default_target_cache_reuse_ratio() -> f64 {
    0.0
}

fn default_virtual_cache_context_shrink_reset_ratio() -> f64 {
    0.7
}

fn default_dynamic_proxy_provider() -> String {
    "novproxy".to_string()
}

fn default_dynamic_proxy_protocol() -> String {
    "http".to_string()
}

fn default_dynamic_proxy_host() -> String {
    "us.novproxy.io".to_string()
}

fn default_dynamic_proxy_port() -> u16 {
    1000
}

fn default_dynamic_proxy_username_template() -> String {
    "nfgr68136-region-{region}-st-{state}-sid-{sid}-t-{ttl}".to_string()
}

fn default_dynamic_proxy_region() -> String {
    "US".to_string()
}

fn default_dynamic_proxy_state() -> String {
    "New Jersey".to_string()
}

fn default_dynamic_proxy_ttl_minutes() -> u32 {
    120
}

fn default_dynamic_proxy_renew_before_ms() -> u64 {
    900_000
}

fn default_dynamic_proxy_verify_url() -> String {
    "https://ipinfo.io/json".to_string()
}

fn default_dynamic_proxy_max_bind_retries() -> u32 {
    3
}

fn default_dynamic_proxy_worker_interval_ms() -> u64 {
    60_000
}

fn default_dynamic_proxy_worker_batch_size() -> usize {
    20
}

fn default_dynamic_proxy_worker_concurrency() -> usize {
    3
}

fn default_shutdown_drain_timeout_secs() -> u64 {
    60
}

fn default_extract_thinking() -> bool {
    true
}

fn default_endpoint() -> String {
    crate::kiro::endpoint::ide::IDE_ENDPOINT_NAME.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            auth_region: None,
            api_region: None,
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            system_version: default_system_version(),
            node_version: default_node_version(),
            request_diagnostics_enabled: false,
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            admin_api_key: None,
            load_balancing_mode: default_load_balancing_mode(),
            global_max_concurrent: default_global_max_concurrent(),
            global_max_concurrent_limit: default_global_max_concurrent_limit(),
            per_account_max_concurrent: default_per_account_max_concurrent(),
            queue_max_size: default_queue_max_size(),
            queue_timeout_ms: default_queue_timeout_ms(),
            per_account_rpm: 0,
            global_rpm: 0,
            rate_limit_cooldown_ms: default_rate_limit_cooldown_ms(),
            transient_cooldown_ms: default_transient_cooldown_ms(),
            max_retry_accounts: default_max_retry_accounts(),
            allow_over_usage: false,
            model_capacity_cooldown_ms: default_model_capacity_cooldown_ms(),
            same_account_retry_rules: default_same_account_retry_rules(),
            token_auto_refresh_enabled: default_token_auto_refresh_enabled(),
            token_auto_refresh_interval_secs: default_token_auto_refresh_interval_secs(),
            token_auto_refresh_window_secs: default_token_auto_refresh_window_secs(),
            session_affinity_enabled: default_session_affinity_enabled(),
            session_affinity_ttl_secs: default_session_affinity_ttl_secs(),
            opus47_plain_stabilization_mode: default_opus47_plain_stabilization_mode(),
            opus47_antml_probe_compat: default_opus47_antml_probe_compat(),
            opus47_clean_probe_mode: default_opus47_clean_probe_mode(),
            opus47_detection_profile: default_opus47_detection_profile(),
            opus47_signed_thinking_preservation: default_opus47_signed_thinking_preservation(),
            opus47_short_thinking_experiment: default_opus47_short_thinking_experiment(),
            opus47_diagnostics_enabled: default_opus47_diagnostics_enabled(),
            opus47_raw_debug_enabled: false,
            opus47_raw_debug_max_chars: default_opus47_raw_debug_max_chars(),
            opus46_detection_profile: default_opus46_detection_profile(),
            opus46_antml_probe_compat: default_opus46_antml_probe_compat(),
            opus46_diagnostics_enabled: default_opus46_diagnostics_enabled(),
            opus46_raw_debug_enabled: false,
            opus46_raw_debug_max_chars: default_opus46_raw_debug_max_chars(),
            sonnet46_detection_profile: default_sonnet46_detection_profile(),
            sonnet46_antml_probe_compat: default_sonnet46_antml_probe_compat(),
            sonnet46_diagnostics_enabled: default_sonnet46_diagnostics_enabled(),
            sonnet46_raw_debug_enabled: false,
            sonnet46_raw_debug_max_chars: default_sonnet46_raw_debug_max_chars(),
            prompt_dump_enabled: false,
            prompt_dump_dir: default_prompt_dump_dir(),
            prompt_dump_max_bytes: default_prompt_dump_max_bytes(),
            prompt_dump_models: default_prompt_dump_models(),
            message_pruning_enabled: false,
            message_pruning_max_request_bytes: default_message_pruning_max_request_bytes(),
            message_pruning_keep_recent_messages: default_message_pruning_keep_recent_messages(),
            message_pruning_max_history_entry_bytes:
                default_message_pruning_max_history_entry_bytes(),
            message_pruning_max_truncated_content_bytes:
                default_message_pruning_max_truncated_content_bytes(),
            compat_usage_shape: default_compat_usage_shape(),
            compat_thinking_model: default_compat_thinking_model(),
            compat_models_shape: default_compat_models_shape(),
            virtual_cache_usage_enabled: default_virtual_cache_usage_enabled(),
            virtual_cache_default_ttl: default_virtual_cache_default_ttl(),
            virtual_cache_uncached_input_tokens: default_virtual_cache_uncached_input_tokens(),
            virtual_cache_input_mode: default_virtual_cache_input_mode(),
            virtual_cache_min_input_tokens: default_virtual_cache_min_input_tokens(),
            virtual_cache_max_input_tokens: default_virtual_cache_max_input_tokens(),
            virtual_cache_warmup_tokens: default_virtual_cache_warmup_tokens(),
            virtual_cache_min_creation_tokens: default_virtual_cache_min_creation_tokens(),
            virtual_cache_max_creation_tokens: default_virtual_cache_max_creation_tokens(),
            virtual_cache_creation_mode: default_virtual_cache_creation_mode(),
            virtual_cache_creation_jitter_ratio: default_virtual_cache_creation_jitter_ratio(),
            virtual_cache_burst_every_turns: default_virtual_cache_burst_every_turns(),
            virtual_cache_burst_min_tokens: default_virtual_cache_burst_min_tokens(),
            virtual_cache_burst_max_tokens: default_virtual_cache_burst_max_tokens(),
            virtual_cache_fallback_scope: default_virtual_cache_fallback_scope(),
            virtual_cache_haiku_input_only_enabled: default_virtual_cache_haiku_input_only_enabled(
            ),
            target_cache_reuse_ratio: default_target_cache_reuse_ratio(),
            virtual_cache_context_shrink_reset_ratio:
                default_virtual_cache_context_shrink_reset_ratio(),
            dynamic_proxy_enabled: false,
            dynamic_proxy_provider: default_dynamic_proxy_provider(),
            dynamic_proxy_protocol: default_dynamic_proxy_protocol(),
            dynamic_proxy_host: default_dynamic_proxy_host(),
            dynamic_proxy_port: default_dynamic_proxy_port(),
            dynamic_proxy_username_template: default_dynamic_proxy_username_template(),
            dynamic_proxy_password: String::new(),
            dynamic_proxy_region: default_dynamic_proxy_region(),
            dynamic_proxy_state: default_dynamic_proxy_state(),
            dynamic_proxy_ttl_minutes: default_dynamic_proxy_ttl_minutes(),
            dynamic_proxy_renew_before_ms: default_dynamic_proxy_renew_before_ms(),
            dynamic_proxy_verify_url: default_dynamic_proxy_verify_url(),
            dynamic_proxy_max_bind_retries: default_dynamic_proxy_max_bind_retries(),
            dynamic_proxy_auto_bind_new_accounts: false,
            dynamic_proxy_worker_interval_ms: default_dynamic_proxy_worker_interval_ms(),
            dynamic_proxy_worker_batch_size: default_dynamic_proxy_worker_batch_size(),
            dynamic_proxy_worker_concurrency: default_dynamic_proxy_worker_concurrency(),
            shutdown_drain_timeout_secs: default_shutdown_drain_timeout_secs(),
            extract_thinking: default_extract_thinking(),
            default_endpoint: default_endpoint(),
            endpoints: HashMap::new(),
            config_path: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 获取有效的 Auth Region（用于 Token 刷新）
    /// 优先使用 auth_region，未配置时回退到 region
    pub fn effective_auth_region(&self) -> &str {
        self.auth_region.as_deref().unwrap_or(&self.region)
    }

    /// 获取有效的 API Region（用于 API 请求）
    /// 优先使用 api_region，未配置时回退到 region
    pub fn effective_api_region(&self) -> &str {
        self.api_region.as_deref().unwrap_or(&self.region)
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            let mut config = Self::default();
            config.config_path = Some(path.to_path_buf());
            return Ok(config);
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        config.config_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// 获取配置文件路径（如果有）
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 将当前配置写回原始配置文件
    pub fn save(&self) -> anyhow::Result<()> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存配置"))?;

        let content = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, content)
            .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }
}
