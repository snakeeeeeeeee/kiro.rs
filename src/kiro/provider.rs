//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试
//! 支持按凭据级 endpoint 切换不同 Kiro API 端点

use futures::{
    StreamExt,
    stream::{self, BoxStream},
};
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::dynamic_proxy::is_proxy_error;
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model_cooldown::ModelCooldownManager;
use crate::kiro::parser::frame::Frame;
use crate::kiro::token_manager::{CredentialLease, MultiTokenManager};
use crate::metrics::{MetricsRecorder, RequestTimingSample, UpstreamOutcome, duration_ms};
use crate::model::config::TlsBackend;
use parking_lot::Mutex;

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    metrics: Arc<MetricsRecorder>,
    model_cooldowns: Arc<ModelCooldownManager>,
    /// Client 缓存：key = effective proxy config, value = reqwest::Client
    /// 不同代理配置的凭据使用不同的 Client，共享相同代理的凭据复用 Client
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
    /// TLS 后端配置
    tls_backend: TlsBackend,
    /// 端点实现注册表（key: endpoint 名称）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称（凭据未指定 endpoint 时使用）
    default_endpoint: String,
}

/// 上游 API 响应及其绑定的凭据占用守卫
pub struct LeasedResponse {
    response: Option<reqwest::Response>,
    credential_id: u64,
    attempts: usize,
    raw_request_id: Option<String>,
    raw_debug_enabled: bool,
    raw_debug_max_chars: usize,
    lease: Option<CredentialLease>,
    timing: Option<ResponseTimingGuard>,
}

struct ApiTimingRecord {
    model: String,
    is_stream: bool,
    credential_id: Option<u64>,
    status: Option<u16>,
    outcome: UpstreamOutcome,
    attempts: usize,
    queue_ms: u64,
    acquire_ms: u64,
    upstream_ms: u64,
    total_ms: u64,
}

#[derive(Debug, Clone)]
struct StreamTimingLogContext {
    model: String,
    credential_id: u64,
    status: u16,
    attempts: usize,
    queue_ms: u64,
    acquire_ms: u64,
    header_ms: u64,
    total_started_at: Instant,
}

#[derive(Debug)]
pub struct ProviderRateLimitError {
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl std::fmt::Display for ProviderRateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProviderRateLimitError {}

struct UpstreamErrorInfo {
    reason: Option<String>,
    retry_after_ms: Option<u64>,
}

#[derive(Debug, Default, Clone)]
struct RequestDiagnostics {
    model: Option<String>,
    conversation_id: Option<String>,
    agent_task_type: Option<String>,
    chat_trigger_type: Option<String>,
    current_content_chars: usize,
    history_len: usize,
    history_user_count: usize,
    history_assistant_count: usize,
    tools_count: usize,
    tool_results_count: usize,
    current_images_count: usize,
    profile_arn_present: bool,
    request_bytes: usize,
    thinking_directives_present: bool,
}

struct ResponseTimingGuard {
    metrics: Arc<MetricsRecorder>,
    record: Option<ApiTimingRecord>,
    total_started_at: Instant,
    body_started_at: Instant,
    stream_log: Option<StreamTimingLogContext>,
    first_chunk_logged: bool,
}

impl ResponseTimingGuard {
    fn new(
        metrics: Arc<MetricsRecorder>,
        record: ApiTimingRecord,
        total_started_at: Instant,
        body_started_at: Instant,
    ) -> Self {
        Self {
            metrics,
            record: Some(record),
            total_started_at,
            body_started_at,
            stream_log: None,
            first_chunk_logged: false,
        }
    }

    fn with_stream_log(mut self, context: StreamTimingLogContext) -> Self {
        self.stream_log = Some(context);
        self
    }

    fn log_first_chunk(&mut self, chunk_len: usize) {
        if self.first_chunk_logged {
            return;
        }
        self.first_chunk_logged = true;

        if let Some(context) = &self.stream_log {
            tracing::info!(
                target: "kiro_rs::metrics",
                model = %context.model,
                stream = true,
                credential_id = context.credential_id,
                status = context.status,
                attempts = context.attempts,
                queue_ms = context.queue_ms,
                acquire_ms = context.acquire_ms,
                header_ms = context.header_ms,
                first_chunk_ms = duration_ms(context.total_started_at.elapsed()),
                first_chunk_body_ms = duration_ms(self.body_started_at.elapsed()),
                chunk_bytes = chunk_len,
                "upstream_stream_first_chunk"
            );
        }
    }

    fn finish(&mut self, outcome: UpstreamOutcome) {
        if let Some(mut record) = self.record.take() {
            record.outcome = outcome;
            record.upstream_ms = record
                .upstream_ms
                .saturating_add(duration_ms(self.body_started_at.elapsed()));
            record.total_ms = duration_ms(self.total_started_at.elapsed());
            record_api_timing(&self.metrics, record);
        }
    }
}

impl Drop for ResponseTimingGuard {
    fn drop(&mut self) {
        self.finish(UpstreamOutcome::Error);
    }
}

impl LeasedResponse {
    fn new(
        response: reqwest::Response,
        credential_id: u64,
        attempts: usize,
        raw_request_id: Option<String>,
        raw_debug_enabled: bool,
        raw_debug_max_chars: usize,
        lease: CredentialLease,
        timing: Option<ResponseTimingGuard>,
    ) -> Self {
        Self {
            response: Some(response),
            credential_id,
            attempts,
            raw_request_id,
            raw_debug_enabled,
            raw_debug_max_chars,
            lease: Some(lease),
            timing,
        }
    }

    pub fn credential_id(&self) -> u64 {
        self.credential_id
    }

    pub fn attempts(&self) -> usize {
        self.attempts
    }

    pub fn raw_request_id(&self) -> Option<&str> {
        self.raw_request_id.as_deref()
    }

    pub fn raw_debug_enabled(&self) -> bool {
        self.raw_debug_enabled
    }

    pub fn raw_debug_max_chars(&self) -> usize {
        self.raw_debug_max_chars
    }

    pub async fn bytes(mut self) -> Result<bytes::Bytes, reqwest::Error> {
        let response = self
            .response
            .take()
            .expect("LeasedResponse body already taken");
        let result = response.bytes().await;
        if let Some(timing) = self.timing.as_mut() {
            timing.finish(if result.is_ok() {
                UpstreamOutcome::Success
            } else {
                UpstreamOutcome::Error
            });
        }
        result
    }

    pub async fn text(mut self) -> Result<String, reqwest::Error> {
        let response = self
            .response
            .take()
            .expect("LeasedResponse body already taken");
        let result = response.text().await;
        if let Some(timing) = self.timing.as_mut() {
            timing.finish(if result.is_ok() {
                UpstreamOutcome::Success
            } else {
                UpstreamOutcome::Error
            });
        }
        result
    }

    pub fn bytes_stream(mut self) -> BoxStream<'static, Result<bytes::Bytes, reqwest::Error>> {
        let response = self
            .response
            .take()
            .expect("LeasedResponse body already taken");
        let body_stream = response.bytes_stream();
        let timing = self.timing.take();
        let lease = self.lease.take();

        stream::unfold(
            (body_stream, timing, lease),
            |(mut body_stream, mut timing, lease)| async move {
                match body_stream.next().await {
                    Some(Ok(chunk)) => {
                        if let Some(timing) = timing.as_mut() {
                            timing.log_first_chunk(chunk.len());
                        }
                        Some((Ok(chunk), (body_stream, timing, lease)))
                    }
                    Some(Err(err)) => {
                        if let Some(timing) = timing.as_mut() {
                            timing.finish(UpstreamOutcome::Error);
                        }
                        Some((Err(err), (body_stream, timing, lease)))
                    }
                    None => {
                        if let Some(timing) = timing.as_mut() {
                            timing.finish(UpstreamOutcome::Success);
                        }
                        drop(lease);
                        None
                    }
                }
            },
        )
        .boxed()
    }
}

fn record_api_timing(metrics: &MetricsRecorder, record: ApiTimingRecord) {
    tracing::info!(
        target: "kiro_rs::metrics",
        model = %record.model,
        stream = record.is_stream,
        credential_id = record.credential_id,
        status = record.status,
        outcome = match record.outcome {
            UpstreamOutcome::Success => "success",
            UpstreamOutcome::Error => "error",
        },
        attempts = record.attempts,
        queue_ms = record.queue_ms,
        acquire_ms = record.acquire_ms,
        upstream_ms = record.upstream_ms,
        total_ms = record.total_ms,
        "upstream_request_timing"
    );
    metrics.record(RequestTimingSample {
        completed_at: chrono::Utc::now(),
        model: record.model,
        stream: record.is_stream,
        credential_id: record.credential_id,
        status: record.status,
        outcome: record.outcome,
        attempts: record.attempts,
        queue_ms: record.queue_ms,
        acquire_ms: record.acquire_ms,
        upstream_ms: record.upstream_ms,
        total_ms: record.total_ms,
    });
}

fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|instant| instant.duration_since(std::time::SystemTime::now()).ok())
        .map(duration_ms)
}

fn extract_upstream_reason(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("reason")
        .and_then(|reason| reason.as_str())
        .map(str::to_string)
}

fn retry_after_secs(retry_after_ms: Option<u64>) -> Option<u64> {
    retry_after_ms.map(|ms| ms.div_ceil(1_000).max(1))
}

fn provider_rate_limit_error(
    message: impl Into<String>,
    retry_after_ms: Option<u64>,
) -> anyhow::Error {
    ProviderRateLimitError {
        message: message.into(),
        retry_after_secs: retry_after_secs(retry_after_ms),
    }
    .into()
}

fn truncate_for_raw_debug(value: &str, max_chars: usize) -> (String, bool) {
    let max_chars = max_chars.max(1);
    let mut truncated = false;
    let mut result = String::new();

    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            truncated = true;
            break;
        }
        result.push(ch);
    }

    (result, truncated)
}

fn is_opus47_raw_debug_model(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "claude-opus-4-7"
            | "claude-opus-4.7"
            | "claude-opus-4-7-thinking"
            | "claude-opus-4.7-thinking"
    )
}

fn log_kiro_raw_request(
    raw_request_id: &str,
    model: &str,
    credential_id: u64,
    endpoint_name: &str,
    url: &str,
    is_stream: bool,
    body: &str,
    max_chars: usize,
) {
    let (body_preview, truncated) = truncate_for_raw_debug(body, max_chars);
    tracing::warn!(
        target: "kiro_rs::raw_debug",
        raw_request_id = raw_request_id,
        model = model,
        credential_id = credential_id,
        endpoint = endpoint_name,
        url = url,
        stream = is_stream,
        body_chars = body.chars().count(),
        body_bytes = body.len(),
        truncated = truncated,
        body = %body_preview,
        "kiro_raw_request"
    );
}

pub(crate) fn log_kiro_raw_stream_chunk(
    raw_request_id: Option<&str>,
    model: &str,
    credential_id: u64,
    chunk_index: usize,
    chunk: &[u8],
    max_chars: usize,
) {
    let Some(raw_request_id) = raw_request_id else {
        return;
    };
    let chunk_text = String::from_utf8_lossy(chunk);
    let (chunk_preview, truncated) = truncate_for_raw_debug(&chunk_text, max_chars);
    tracing::warn!(
        target: "kiro_rs::raw_debug",
        raw_request_id = raw_request_id,
        model = model,
        credential_id = credential_id,
        chunk_index = chunk_index,
        chunk_bytes = chunk.len(),
        truncated = truncated,
        chunk = %chunk_preview,
        "kiro_raw_stream_chunk"
    );
}

pub(crate) fn log_kiro_raw_stream_frame(
    raw_request_id: Option<&str>,
    model: &str,
    credential_id: u64,
    frame_index: usize,
    frame: &Frame,
    max_chars: usize,
) {
    let Some(raw_request_id) = raw_request_id else {
        return;
    };
    let payload = String::from_utf8_lossy(&frame.payload);
    let (payload_preview, truncated) = truncate_for_raw_debug(&payload, max_chars);
    tracing::warn!(
        target: "kiro_rs::raw_debug",
        raw_request_id = raw_request_id,
        model = model,
        credential_id = credential_id,
        frame_index = frame_index,
        message_type = frame.message_type(),
        event_type = frame.event_type(),
        error_code = frame.headers.error_code(),
        exception_type = frame.headers.exception_type(),
        payload_bytes = frame.payload.len(),
        truncated = truncated,
        payload = %payload_preview,
        "kiro_raw_stream_frame"
    );
}

pub(crate) fn log_kiro_raw_parsed_event(
    raw_request_id: Option<&str>,
    model: &str,
    credential_id: u64,
    event_index: usize,
    event_type: &str,
    event_debug: &str,
    max_chars: usize,
) {
    let Some(raw_request_id) = raw_request_id else {
        return;
    };
    let (event_preview, truncated) = truncate_for_raw_debug(event_debug, max_chars);
    tracing::warn!(
        target: "kiro_rs::raw_debug",
        raw_request_id = raw_request_id,
        model = model,
        credential_id = credential_id,
        event_index = event_index,
        event_type = event_type,
        truncated = truncated,
        event = %event_preview,
        "kiro_raw_parsed_event"
    );
}

pub(crate) fn log_kiro_raw_nonstream_body(
    raw_request_id: Option<&str>,
    model: &str,
    credential_id: u64,
    body: &[u8],
    max_chars: usize,
) {
    let Some(raw_request_id) = raw_request_id else {
        return;
    };
    let body_text = String::from_utf8_lossy(body);
    let (body_preview, truncated) = truncate_for_raw_debug(&body_text, max_chars);
    tracing::warn!(
        target: "kiro_rs::raw_debug",
        raw_request_id = raw_request_id,
        model = model,
        credential_id = credential_id,
        body_bytes = body.len(),
        truncated = truncated,
        body = %body_preview,
        "kiro_raw_nonstream_body"
    );
}

impl KiroProvider {
    /// 创建带代理配置和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        metrics: Arc<MetricsRecorder>,
        model_cooldowns: Arc<ModelCooldownManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );
        let tls_backend = token_manager.config().tls_backend;
        // 预热：构建全局代理对应的 Client
        let initial_client =
            build_client(proxy.as_ref(), 720, tls_backend).expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            metrics,
            model_cooldowns,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
        }
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(
        &self,
        credential_id: u64,
        credentials: &KiroCredentials,
    ) -> anyhow::Result<Client> {
        let effective = self
            .token_manager
            .effective_proxy_for(credential_id, credentials);
        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok(client.clone());
        }
        let client = build_client(effective.as_ref(), 720, self.tls_backend)?;
        cache.insert(effective, client.clone());
        Ok(client)
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）
    #[allow(dead_code)]
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, false, None, 0).await
    }

    pub async fn call_api_with_session(
        &self,
        request_body: &str,
        session_id: Option<&str>,
        queue_ms: u64,
    ) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, false, session_id, queue_ms)
            .await
    }

    /// 发送流式 API 请求
    #[allow(dead_code)]
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, true, None, 0).await
    }

    pub async fn call_api_stream_with_session(
        &self,
        request_body: &str,
        session_id: Option<&str>,
        queue_ms: u64,
    ) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, true, session_id, queue_ms)
            .await
    }

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    #[allow(dead_code)]
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<LeasedResponse> {
        self.call_mcp_with_retry(request_body, None).await
    }

    pub async fn call_mcp_with_session(
        &self,
        request_body: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<LeasedResponse> {
        self.call_mcp_with_retry(request_body, session_id).await
    }

    pub fn token_manager(&self) -> Arc<MultiTokenManager> {
        self.token_manager.clone()
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(
        &self,
        request_body: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<LeasedResponse> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self
                .token_manager
                .acquire_context_with_session(None, session_id)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    // endpoint 解析失败：记为失败，换下一张凭据
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.mcp_url(&rctx);
            let body = endpoint.transform_mcp_body(request_body, &rctx);

            let base = self
                .client_for(ctx.id, &ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json");
            let request = endpoint.decorate_mcp(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    if is_proxy_error(&e) {
                        self.mark_dynamic_proxy_failure(ctx.id, &e).await;
                    }
                    self.token_manager.report_transient_error(ctx.id);
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                return Ok(LeasedResponse::new(
                    response,
                    ctx.id,
                    attempt + 1,
                    None,
                    false,
                    0,
                    ctx.lease,
                    None,
                ));
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                if status.as_u16() == 429 {
                    self.token_manager.report_rate_limited(ctx.id);
                } else {
                    self.token_manager.report_transient_error(ctx.id);
                }
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
        session_id: Option<&str>,
        queue_ms: u64,
    ) -> anyhow::Result<LeasedResponse> {
        let total_started_at = Instant::now();
        let total_credentials = self.token_manager.total_count();
        let settings = self.token_manager.runtime_settings();
        let max_retry_accounts = settings.max_retry_accounts.min(total_credentials).max(1);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();
        let mut excluded_credentials: HashSet<u64> = HashSet::new();
        let mut attempted_credentials: Vec<u64> = Vec::new();
        let mut http_attempts = 0usize;
        let mut model_capacity_failures = 0usize;
        let mut last_retry_after_ms: Option<u64> = None;
        let api_type = if is_stream { "流式" } else { "非流式" };

        // 尝试从请求体中提取模型信息
        let model = Self::extract_model_from_request(request_body);
        let model_for_metrics = model.clone().unwrap_or_else(|| "unknown".to_string());
        if let Some(cooldown) = self.model_cooldowns.check(&model_for_metrics) {
            let retry_after_ms = Some(cooldown.remaining_ms);
            let total_ms = duration_ms(total_started_at.elapsed());
            self.record_api_timing(ApiTimingRecord {
                model: model_for_metrics.clone(),
                is_stream,
                credential_id: None,
                status: Some(429),
                outcome: UpstreamOutcome::Error,
                attempts: 0,
                queue_ms,
                acquire_ms: 0,
                upstream_ms: 0,
                total_ms,
            });
            return Err(provider_rate_limit_error(
                format!(
                    "{} API 请求失败：模型 {} 正在冷却，请稍后重试",
                    api_type, model_for_metrics
                ),
                retry_after_ms,
            ));
        }
        let mut last_credential_id: Option<u64> = None;
        let mut last_status: Option<u16> = None;
        let mut acquire_ms_total = 0;
        let mut upstream_ms_total = 0;

        loop {
            if attempted_credentials.len() >= max_retry_accounts {
                break;
            }

            // 获取调用上下文（绑定 index、credentials、token）
            let acquire_started_at = Instant::now();
            let ctx = match self
                .token_manager
                .acquire_context_with_session_excluding(
                    model.as_deref(),
                    session_id,
                    &excluded_credentials,
                )
                .await
            {
                Ok(c) => {
                    acquire_ms_total += duration_ms(acquire_started_at.elapsed());
                    c
                }
                Err(e) => {
                    acquire_ms_total += duration_ms(acquire_started_at.elapsed());
                    last_error = Some(e);
                    break;
                }
            };
            last_credential_id = Some(ctx.id);
            if !attempted_credentials.contains(&ctx.id) {
                attempted_credentials.push(ctx.id);
            }

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.api_url(&rctx);
            let body = endpoint.transform_api_body(request_body, &rctx);
            let raw_debug_enabled =
                settings.opus47_raw_debug_enabled && is_opus47_raw_debug_model(&model_for_metrics);
            let raw_debug_max_chars = settings.opus47_raw_debug_max_chars;
            let raw_request_id = raw_debug_enabled.then(|| Uuid::new_v4().to_string());
            if let Some(raw_request_id) = raw_request_id.as_deref() {
                log_kiro_raw_request(
                    raw_request_id,
                    &model_for_metrics,
                    ctx.id,
                    endpoint.name(),
                    &url,
                    is_stream,
                    &body,
                    raw_debug_max_chars,
                );
            }
            if config.request_diagnostics_enabled {
                let request_diagnostics = Self::diagnose_api_request_body(&body);
                tracing::info!(
                    model = request_diagnostics.model.as_deref().unwrap_or("unknown"),
                    external_model = model_for_metrics.as_str(),
                    credential_id = ctx.id,
                    endpoint = endpoint.name(),
                    api_region = rctx.credentials.effective_api_region(config),
                    kiro_version = config.kiro_version.as_str(),
                    node_version = config.node_version.as_str(),
                    system_version = config.system_version.as_str(),
                    stream = is_stream,
                    conversation_id = request_diagnostics.conversation_id.as_deref(),
                    agent_task_type = request_diagnostics.agent_task_type.as_deref(),
                    chat_trigger_type = request_diagnostics.chat_trigger_type.as_deref(),
                    history_len = request_diagnostics.history_len,
                    history_user_count = request_diagnostics.history_user_count,
                    history_assistant_count = request_diagnostics.history_assistant_count,
                    current_content_chars = request_diagnostics.current_content_chars,
                    tools_count = request_diagnostics.tools_count,
                    tool_results_count = request_diagnostics.tool_results_count,
                    current_images_count = request_diagnostics.current_images_count,
                    profile_arn_present = request_diagnostics.profile_arn_present,
                    request_bytes = request_diagnostics.request_bytes,
                    thinking_directives_present = request_diagnostics.thinking_directives_present,
                    "kiro_api_request_diagnostics"
                );
            }

            let base = self
                .client_for(ctx.id, &ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json");
            let request = endpoint.decorate_api(base, &rctx);

            let upstream_started_at = Instant::now();
            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    upstream_ms_total += duration_ms(upstream_started_at.elapsed());
                    tracing::warn!(
                        credential_id = ctx.id,
                        attempted_credentials = ?attempted_credentials,
                        excluded_credential_ids = ?excluded_credentials,
                        "API 请求发送失败（尝试账号 {}/{}）: {}",
                        attempted_credentials.len(),
                        max_retry_accounts,
                        e
                    );
                    if is_proxy_error(&e) {
                        self.mark_dynamic_proxy_failure(ctx.id, &e).await;
                    }
                    self.token_manager
                        .report_transient_error_for(ctx.id, settings.transient_cooldown_ms);
                    excluded_credentials.insert(ctx.id);
                    last_error = Some(e.into());
                    if attempted_credentials.len() < max_retry_accounts {
                        sleep(Self::retry_delay(http_attempts)).await;
                    }
                    continue;
                }
            };
            http_attempts += 1;
            upstream_ms_total += duration_ms(upstream_started_at.elapsed());

            let status = response.status();
            last_status = Some(status.as_u16());
            let retry_after_ms = parse_retry_after_ms(response.headers());

            // 成功响应：耗时指标延后到响应体消费完成时记录。
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                let mut timing = ResponseTimingGuard::new(
                    self.metrics.clone(),
                    ApiTimingRecord {
                        model: model_for_metrics.clone(),
                        is_stream,
                        credential_id: Some(ctx.id),
                        status: Some(status.as_u16()),
                        outcome: UpstreamOutcome::Success,
                        attempts: http_attempts,
                        queue_ms,
                        acquire_ms: acquire_ms_total,
                        upstream_ms: upstream_ms_total,
                        total_ms: 0,
                    },
                    total_started_at,
                    Instant::now(),
                );
                if is_stream {
                    timing = timing.with_stream_log(StreamTimingLogContext {
                        model: model_for_metrics.clone(),
                        credential_id: ctx.id,
                        status: status.as_u16(),
                        attempts: http_attempts,
                        queue_ms,
                        acquire_ms: acquire_ms_total,
                        header_ms: upstream_ms_total,
                        total_started_at,
                    });
                }
                return Ok(LeasedResponse::new(
                    response,
                    ctx.id,
                    http_attempts,
                    raw_request_id,
                    raw_debug_enabled,
                    raw_debug_max_chars,
                    ctx.lease,
                    Some(timing),
                ));
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body_started_at = Instant::now();
            let body = response.text().await.unwrap_or_default();
            upstream_ms_total =
                upstream_ms_total.saturating_add(duration_ms(body_started_at.elapsed()));
            let upstream_error = UpstreamErrorInfo {
                reason: extract_upstream_reason(&body),
                retry_after_ms,
            };
            last_retry_after_ms = upstream_error.retry_after_ms.or(last_retry_after_ms);

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                tracing::warn!(
                    credential_id = ctx.id,
                    attempted_credentials = ?attempted_credentials,
                    excluded_credential_ids = ?excluded_credentials,
                    "API 请求失败（额度已用尽，禁用凭据并切换，尝试账号 {}/{}）: {} {}",
                    attempted_credentials.len(),
                    max_retry_accounts,
                    status,
                    body
                );

                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    let total_ms = duration_ms(total_started_at.elapsed());
                    self.record_api_timing(ApiTimingRecord {
                        model: model_for_metrics,
                        is_stream,
                        credential_id: Some(ctx.id),
                        status: Some(status.as_u16()),
                        outcome: UpstreamOutcome::Error,
                        attempts: http_attempts,
                        queue_ms,
                        acquire_ms: acquire_ms_total,
                        upstream_ms: upstream_ms_total,
                        total_ms,
                    });
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                excluded_credentials.insert(ctx.id);
                continue;
            }

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                let total_ms = duration_ms(total_started_at.elapsed());
                self.record_api_timing(ApiTimingRecord {
                    model: model_for_metrics.clone(),
                    is_stream,
                    credential_id: Some(ctx.id),
                    status: Some(status.as_u16()),
                    outcome: UpstreamOutcome::Error,
                    attempts: http_attempts,
                    queue_ms,
                    acquire_ms: acquire_ms_total,
                    upstream_ms: upstream_ms_total,
                    total_ms,
                });
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 401/403 - 更可能是凭据/权限问题：计入失败并允许故障转移
            if matches!(status.as_u16(), 401 | 403) {
                tracing::warn!(
                    credential_id = ctx.id,
                    upstream_reason = upstream_error.reason.as_deref(),
                    attempted_credentials = ?attempted_credentials,
                    excluded_credential_ids = ?excluded_credentials,
                    "API 请求失败（可能为凭据错误，尝试账号 {}/{}）: {} {}",
                    attempted_credentials.len(),
                    max_retry_accounts,
                    status,
                    body
                );

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        attempted_credentials.retain(|id| *id != ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    let total_ms = duration_ms(total_started_at.elapsed());
                    self.record_api_timing(ApiTimingRecord {
                        model: model_for_metrics,
                        is_stream,
                        credential_id: Some(ctx.id),
                        status: Some(status.as_u16()),
                        outcome: UpstreamOutcome::Error,
                        attempts: http_attempts,
                        queue_ms,
                        acquire_ms: acquire_ms_total,
                        upstream_ms: upstream_ms_total,
                        total_ms,
                    });
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                excluded_credentials.insert(ctx.id);
                continue;
            }

            // 429/408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            // （避免 429 high traffic / 502 high load 等瞬态错误把所有凭据锁死）
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                let is_model_capacity = status.as_u16() == 429
                    && upstream_error.reason.as_deref() == Some("INSUFFICIENT_MODEL_CAPACITY");
                if is_model_capacity {
                    model_capacity_failures += 1;
                    last_retry_after_ms = upstream_error.retry_after_ms.or(last_retry_after_ms);
                } else if status.as_u16() == 429 {
                    let cooldown_ms = upstream_error
                        .retry_after_ms
                        .unwrap_or(settings.rate_limit_cooldown_ms);
                    self.token_manager
                        .report_rate_limited_for(ctx.id, cooldown_ms);
                } else {
                    self.token_manager
                        .report_transient_error_for(ctx.id, settings.transient_cooldown_ms);
                }
                tracing::warn!(
                    credential_id = ctx.id,
                    upstream_reason = upstream_error.reason.as_deref(),
                    retry_after_ms = upstream_error.retry_after_ms,
                    cooldown_scope = if is_model_capacity { "model" } else { "account" },
                    attempted_credentials = ?attempted_credentials,
                    excluded_credential_ids = ?excluded_credentials,
                    "API 请求失败（上游瞬态错误，尝试账号 {}/{}）: {} {}",
                    attempted_credentials.len(),
                    max_retry_accounts,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                excluded_credentials.insert(ctx.id);
                if attempted_credentials.len() < max_retry_accounts {
                    sleep(Self::retry_delay(http_attempts)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                let total_ms = duration_ms(total_started_at.elapsed());
                self.record_api_timing(ApiTimingRecord {
                    model: model_for_metrics.clone(),
                    is_stream,
                    credential_id: Some(ctx.id),
                    status: Some(status.as_u16()),
                    outcome: UpstreamOutcome::Error,
                    attempts: http_attempts,
                    queue_ms,
                    acquire_ms: acquire_ms_total,
                    upstream_ms: upstream_ms_total,
                    total_ms,
                });
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 兜底：当作可重试的瞬态错误处理（不切换凭据）
            tracing::warn!(
                credential_id = ctx.id,
                upstream_reason = upstream_error.reason.as_deref(),
                attempted_credentials = ?attempted_credentials,
                excluded_credential_ids = ?excluded_credentials,
                "API 请求失败（未知错误，尝试账号 {}/{}）: {} {}",
                attempted_credentials.len(),
                max_retry_accounts,
                status,
                body
            );
            last_error = Some(anyhow::anyhow!(
                "{} API 请求失败: {} {}",
                api_type,
                status,
                body
            ));
            excluded_credentials.insert(ctx.id);
            if attempted_credentials.len() < max_retry_accounts {
                sleep(Self::retry_delay(http_attempts)).await;
            }
        }

        if model_capacity_failures > 0 && model_capacity_failures == attempted_credentials.len() {
            let cooldown_ms = last_retry_after_ms.unwrap_or(settings.model_capacity_cooldown_ms);
            self.model_cooldowns.set_cooldown(
                &model_for_metrics,
                cooldown_ms,
                "INSUFFICIENT_MODEL_CAPACITY",
            );
            last_error = Some(provider_rate_limit_error(
                format!(
                    "{} API 请求失败：模型 {} 容量不足，请稍后重试",
                    api_type, model_for_metrics
                ),
                Some(cooldown_ms),
            ));
        }

        // 所有重试都失败
        let error = last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大尝试账号数（{}个）",
                api_type,
                max_retry_accounts
            )
        });
        let total_ms = duration_ms(total_started_at.elapsed());
        self.record_api_timing(ApiTimingRecord {
            model: model_for_metrics,
            is_stream,
            credential_id: last_credential_id,
            status: last_status,
            outcome: UpstreamOutcome::Error,
            attempts: http_attempts,
            queue_ms,
            acquire_ms: acquire_ms_total,
            upstream_ms: upstream_ms_total,
            total_ms,
        });
        Err(error)
    }

    fn record_api_timing(&self, record: ApiTimingRecord) {
        record_api_timing(&self.metrics, record);
    }

    async fn mark_dynamic_proxy_failure(&self, credential_id: u64, err: &reqwest::Error) {
        if let Some(manager) = self.token_manager.dynamic_proxy() {
            let settings = self.token_manager.runtime_settings();
            if let Err(mark_err) = manager
                .mark_failure(credential_id, err, &settings, true)
                .await
            {
                tracing::warn!(
                    credential_id,
                    error = %mark_err,
                    "标记动态代理失败时出错"
                );
            }
        }
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，提取 conversationState.currentMessage.userInputMessage.modelId
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        json.get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn diagnose_api_request_body(request_body: &str) -> RequestDiagnostics {
        use serde_json::Value;

        let mut diagnostics = RequestDiagnostics {
            request_bytes: request_body.len(),
            thinking_directives_present: request_body.contains("<thinking_mode>"),
            ..Default::default()
        };

        let Ok(json) = serde_json::from_str::<Value>(request_body) else {
            return diagnostics;
        };

        diagnostics.profile_arn_present = json.get("profileArn").and_then(Value::as_str).is_some();

        let Some(state) = json.get("conversationState") else {
            return diagnostics;
        };

        diagnostics.conversation_id = state
            .get("conversationId")
            .and_then(Value::as_str)
            .map(str::to_string);
        diagnostics.agent_task_type = state
            .get("agentTaskType")
            .and_then(Value::as_str)
            .map(str::to_string);
        diagnostics.chat_trigger_type = state
            .get("chatTriggerType")
            .and_then(Value::as_str)
            .map(str::to_string);

        if let Some(history) = state.get("history").and_then(Value::as_array) {
            diagnostics.history_len = history.len();
            for item in history {
                if item.get("userInputMessage").is_some() {
                    diagnostics.history_user_count += 1;
                }
                if item.get("assistantResponseMessage").is_some() {
                    diagnostics.history_assistant_count += 1;
                }
            }
        }

        let Some(user_input) = state
            .get("currentMessage")
            .and_then(|v| v.get("userInputMessage"))
        else {
            return diagnostics;
        };

        diagnostics.model = user_input
            .get("modelId")
            .and_then(Value::as_str)
            .map(str::to_string);
        diagnostics.current_content_chars = user_input
            .get("content")
            .and_then(Value::as_str)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        diagnostics.current_images_count = user_input
            .get("images")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        if let Some(context) = user_input.get("userInputMessageContext") {
            diagnostics.tools_count = context
                .get("tools")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            diagnostics.tool_results_count = context
                .get("toolResults")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
        }

        diagnostics
    }

    fn retry_delay(attempt: usize) -> Duration {
        // 指数退避 + 少量抖动，避免上游抖动时放大故障
        const BASE_MS: u64 = 200;
        const MAX_MS: u64 = 2_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    #[test]
    fn retry_after_seconds_header_is_parsed() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(parse_retry_after_ms(&headers), Some(7_000));
    }

    #[test]
    fn retry_after_http_date_header_is_parsed() {
        let retry_at = std::time::SystemTime::now() + std::time::Duration::from_secs(3);
        let retry_at = httpdate::fmt_http_date(retry_at);
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_str(&retry_at).unwrap());

        let parsed = parse_retry_after_ms(&headers).expect("Retry-After date should parse");
        assert!(parsed <= 3_000);
        assert!(parsed > 0);
    }

    #[test]
    fn upstream_reason_is_extracted_from_json_body() {
        assert_eq!(
            extract_upstream_reason(
                r#"{"message":"I am experiencing high traffic","reason":"INSUFFICIENT_MODEL_CAPACITY"}"#
            )
            .as_deref(),
            Some("INSUFFICIENT_MODEL_CAPACITY")
        );
    }

    #[test]
    fn api_request_diagnostics_extracts_safe_summary() {
        let body = r#"{
            "conversationState": {
                "conversationId": "conv-1",
                "agentTaskType": "vibe",
                "chatTriggerType": "MANUAL",
                "history": [
                    {"userInputMessage": {"content": "old", "modelId": "claude-opus-4.6"}},
                    {"assistantResponseMessage": {"content": "ok"}}
                ],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "<thinking_mode>adaptive</thinking_mode>hello",
                        "modelId": "claude-opus-4.7",
                        "images": [{"format": "png", "source": {"bytes": "abc"}}],
                        "userInputMessageContext": {
                            "tools": [{"toolSpecification": {"name": "read"}}],
                            "toolResults": [{"toolUseId": "toolu_1"}]
                        }
                    }
                }
            },
            "profileArn": "arn:test"
        }"#;

        let summary = KiroProvider::diagnose_api_request_body(body);
        assert_eq!(summary.model.as_deref(), Some("claude-opus-4.7"));
        assert_eq!(summary.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(summary.history_len, 2);
        assert_eq!(summary.history_user_count, 1);
        assert_eq!(summary.history_assistant_count, 1);
        assert_eq!(summary.tools_count, 1);
        assert_eq!(summary.tool_results_count, 1);
        assert_eq!(summary.current_images_count, 1);
        assert!(summary.profile_arn_present);
        assert!(summary.thinking_directives_present);
        assert!(summary.request_bytes > 0);
    }
}
