use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

    /// 模型容量不足后的模型级冷却时间（毫秒）
    #[serde(default = "default_model_capacity_cooldown_ms")]
    pub model_capacity_cooldown_ms: u64,

    /// 是否启用后台 Token 自动刷新
    #[serde(default = "default_token_auto_refresh_enabled")]
    pub token_auto_refresh_enabled: bool,

    /// 后台 Token 自动刷新扫描间隔（秒）
    #[serde(default = "default_token_auto_refresh_interval_secs")]
    pub token_auto_refresh_interval_secs: u64,

    /// Token 距离过期多少秒内触发后台刷新
    #[serde(default = "default_token_auto_refresh_window_secs")]
    pub token_auto_refresh_window_secs: u64,

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

    /// 无 metadata 时的 fallback 范围："model" 或 "none"
    #[serde(default = "default_virtual_cache_fallback_scope")]
    pub virtual_cache_fallback_scope: String,

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
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 端点特定的配置
    ///
    /// 键为端点名（如 "ide" / "cli"），值为该端点自由定义的参数对象。
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
    "0.11.107".to_string()
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

fn default_token_auto_refresh_enabled() -> bool {
    true
}

fn default_token_auto_refresh_interval_secs() -> u64 {
    300
}

fn default_token_auto_refresh_window_secs() -> u64 {
    1_800
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
    "model".to_string()
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
            per_account_max_concurrent: default_per_account_max_concurrent(),
            queue_max_size: default_queue_max_size(),
            queue_timeout_ms: default_queue_timeout_ms(),
            per_account_rpm: 0,
            global_rpm: 0,
            rate_limit_cooldown_ms: default_rate_limit_cooldown_ms(),
            transient_cooldown_ms: default_transient_cooldown_ms(),
            max_retry_accounts: default_max_retry_accounts(),
            model_capacity_cooldown_ms: default_model_capacity_cooldown_ms(),
            token_auto_refresh_enabled: default_token_auto_refresh_enabled(),
            token_auto_refresh_interval_secs: default_token_auto_refresh_interval_secs(),
            token_auto_refresh_window_secs: default_token_auto_refresh_window_secs(),
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
