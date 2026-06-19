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

use crate::common::request_log::RequestLogContext;
use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::dynamic_proxy::is_proxy_error;
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::message_pruning::{
    MessagePruningConfig, MessagePruningOutcome, MessagePruningStats, guard_kiro_payload,
};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, UserInputMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::model_cooldown::ModelCooldownManager;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::parser::frame::Frame;
use crate::kiro::prompt_dump::{PromptDump, PromptDumpMetaUpdate};
use crate::kiro::settings::{
    CredentialPolicy, SameAccountRetryRule, matching_same_account_retry_rule,
};
use crate::kiro::token_manager::{CallContext, CredentialLease, MultiTokenManager};
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
}

/// 上游 API 响应及其绑定的凭据占用守卫
pub struct LeasedResponse {
    response: Option<reqwest::Response>,
    credential_id: u64,
    attempts: usize,
    raw_request_id: Option<String>,
    raw_debug_enabled: bool,
    raw_debug_max_chars: usize,
    prompt_dump: Option<PromptDump>,
    lease: Option<CredentialLease>,
    timing: Option<ResponseTimingGuard>,
}

#[derive(Debug, Clone)]
pub struct CredentialTestResult {
    pub credential_id: u64,
    pub model: String,
    pub prompt: String,
    pub response_text: String,
    pub status: u16,
    pub latency_ms: u64,
    pub endpoint: String,
    pub api_region: String,
}

struct FixedCredentialApiResponse {
    response: LeasedResponse,
    endpoint: String,
    api_region: String,
    status: u16,
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
    request_log: Option<RequestLogContext>,
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
    request_log: Option<RequestLogContext>,
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

struct ApiRequestParts {
    endpoint: Arc<dyn KiroEndpoint>,
    endpoint_name: String,
    url: String,
    body: String,
    api_region: String,
    raw_request_id: Option<String>,
    raw_debug_enabled: bool,
    raw_debug_max_chars: usize,
}

impl ApiRequestParts {
    fn replace_body(&mut self, body: String) {
        self.body = body;
    }
}

struct ApiSendSuccess {
    response: reqwest::Response,
    ctx: CallContext,
    status: u16,
    upstream_ms: u64,
    turbo_attempt_index: Option<usize>,
}

struct ApiSendFailure {
    ctx: CallContext,
    status: Option<u16>,
    body: String,
    upstream_error: UpstreamErrorInfo,
    error: Option<anyhow::Error>,
    upstream_ms: u64,
    turbo_attempt_index: Option<usize>,
}

enum ApiSendOutcome {
    Success(ApiSendSuccess),
    Failure(ApiSendFailure),
}

struct ApiSendBatch {
    outcome: ApiSendOutcome,
    actual_fanout: usize,
    upstream_ms: u64,
}

#[derive(Debug, Default, Clone)]
struct RequestDiagnostics {
    model: Option<String>,
    conversation_id: Option<String>,
    agent_task_type: Option<String>,
    chat_trigger_type: Option<String>,
    current_content_chars: usize,
    current_message_bytes: usize,
    history_len: usize,
    history_user_count: usize,
    history_assistant_count: usize,
    history_bytes: usize,
    largest_history_entry_bytes: usize,
    tools_count: usize,
    tools_bytes: usize,
    largest_tool_bytes: usize,
    tool_results_count: usize,
    tool_results_bytes: usize,
    largest_tool_result_bytes: usize,
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
            let request_log = context.request_log.as_ref();
            tracing::info!(
                target: "kiro_rs::metrics",
                request_id = request_log.map_or("", |log| log.request_id.as_str()),
                route = request_log.map_or("", |log| log.route),
                client_device_id = request_log.map_or("", RequestLogContext::client_device_id_for_log),
                client_account_uuid = request_log.map_or("", RequestLogContext::client_account_uuid_for_log),
                client_user = request_log.map_or("", RequestLogContext::client_user_for_log),
                client_session_id = request_log.map_or("", RequestLogContext::client_session_id_for_log),
                usage_session_key = request_log.map_or("", |log| log.usage_session_key.as_str()),
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
        prompt_dump: Option<PromptDump>,
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
            prompt_dump,
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
        if let (Ok(bytes), Some(dump)) = (&result, self.prompt_dump.as_ref()) {
            dump.write_text(
                "upstream_response.raw",
                &String::from_utf8_lossy(bytes.as_ref()),
            );
        }
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
        if let (Ok(text), Some(dump)) = (&result, self.prompt_dump.as_ref()) {
            dump.write_text("upstream_response.raw", text);
        }
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
    let request_log = record.request_log.as_ref();
    tracing::info!(
        target: "kiro_rs::metrics",
        request_id = request_log.map_or("", |log| log.request_id.as_str()),
        route = request_log.map_or("", |log| log.route),
        client_device_id = request_log.map_or("", RequestLogContext::client_device_id_for_log),
        client_account_uuid = request_log.map_or("", RequestLogContext::client_account_uuid_for_log),
        client_user = request_log.map_or("", RequestLogContext::client_user_for_log),
        client_session_id = request_log.map_or("", RequestLogContext::client_session_id_for_log),
        usage_session_key = request_log.map_or("", |log| log.usage_session_key.as_str()),
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

fn same_account_retry_key(
    credential_id: u64,
    status: u16,
    reason: Option<&str>,
    rule: &SameAccountRetryRule,
) -> String {
    format!(
        "{}:{}:{}:{}",
        credential_id,
        status,
        reason.unwrap_or(""),
        rule.status
    )
}

fn turbo_failure_priority(status: Option<u16>) -> u16 {
    match status {
        Some(402) => 600,
        Some(401 | 403) => 590,
        Some(429) => 580,
        Some(408) => 570,
        Some(status) if (500..=599).contains(&status) => 560,
        Some(400) => 550,
        Some(status) => status,
        None => 540,
    }
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

fn raw_debug_config_for_model(
    settings: &crate::kiro::settings::RuntimeSettings,
    model: &str,
) -> (bool, usize) {
    match model.trim().to_ascii_lowercase().as_str() {
        "claude-opus-4-8"
        | "claude-opus-4.8"
        | "claude-opus-4-8-thinking"
        | "claude-opus-4.8-thinking" => (
            settings.opus47_raw_debug_enabled,
            settings.opus47_raw_debug_max_chars,
        ),
        "claude-opus-4-7"
        | "claude-opus-4.7"
        | "claude-opus-4-7-thinking"
        | "claude-opus-4.7-thinking" => (
            settings.opus47_raw_debug_enabled,
            settings.opus47_raw_debug_max_chars,
        ),
        "claude-opus-4-6"
        | "claude-opus-4.6"
        | "claude-opus-4-6-thinking"
        | "claude-opus-4.6-thinking" => (
            settings.opus46_raw_debug_enabled,
            settings.opus46_raw_debug_max_chars,
        ),
        "claude-sonnet-4-6"
        | "claude-sonnet-4.6"
        | "claude-sonnet-4-6-thinking"
        | "claude-sonnet-4.6-thinking" => (
            settings.sonnet46_raw_debug_enabled,
            settings.sonnet46_raw_debug_max_chars,
        ),
        _ => (false, settings.opus47_raw_debug_max_chars),
    }
}

fn map_admin_test_model(model: &str) -> Option<String> {
    let model_lower = model.trim().to_ascii_lowercase();

    if model_lower.contains("sonnet") {
        if model_lower.contains("4-6") || model_lower.contains("4.6") {
            Some("claude-sonnet-4.6".to_string())
        } else {
            Some("claude-sonnet-4.5".to_string())
        }
    } else if model_lower.contains("opus") {
        if model_lower.contains("4-8") || model_lower.contains("4.8") {
            Some("claude-opus-4.8".to_string())
        } else if model_lower.contains("4-7") || model_lower.contains("4.7") {
            Some("claude-opus-4.7".to_string())
        } else if model_lower.contains("4-5") || model_lower.contains("4.5") {
            Some("claude-opus-4.5".to_string())
        } else {
            Some("claude-opus-4.6".to_string())
        }
    } else if model_lower.contains("haiku") {
        Some("claude-haiku-4.5".to_string())
    } else if matches!(
        model_lower.as_str(),
        "deepseek-3.2" | "minimax-m2.5" | "minimax-m2.1" | "glm-5" | "qwen3-coder-next"
    ) {
        Some(model_lower)
    } else {
        None
    }
}

fn parse_assistant_response_text(body: &[u8]) -> String {
    let mut decoder = EventStreamDecoder::new();
    if let Err(err) = decoder.feed(body) {
        tracing::warn!(error = %err, body_bytes = body.len(), "解析账号测试响应失败");
        return String::new();
    }

    let mut text = String::new();
    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(Event::AssistantResponse(event)) => text.push_str(&event.content),
                Ok(Event::Error {
                    error_code,
                    error_message,
                }) => {
                    if !error_message.is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&format!("{}: {}", error_code, error_message));
                    }
                }
                Ok(Event::Exception {
                    exception_type,
                    message,
                }) => {
                    if !message.is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&format!("{}: {}", exception_type, message));
                    }
                }
                Ok(_) => {}
                Err(err) => tracing::debug!(error = %err, "解析账号测试事件失败"),
            },
            Err(err) => tracing::debug!(error = %err, "解析账号测试 event-stream 帧失败"),
        }
    }
    text
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
        let runtime_default_endpoint = self.token_manager.default_endpoint();
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(runtime_default_endpoint.as_str());
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    fn build_api_request_parts(
        &self,
        ctx: &CallContext,
        request_body: &str,
        _is_stream: bool,
        model_for_metrics: &str,
        settings: &crate::kiro::settings::RuntimeSettings,
    ) -> anyhow::Result<ApiRequestParts> {
        let config = self.token_manager.config();
        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);
        let endpoint = self.endpoint_for(&ctx.credentials)?;
        let rctx = RequestContext {
            credentials: &ctx.credentials,
            token: &ctx.token,
            machine_id: &machine_id,
            config,
        };
        let url = endpoint.api_url(&rctx);
        let body = endpoint.transform_api_body(request_body, &rctx);
        let endpoint_name = endpoint.name().to_string();
        let api_region = rctx.credentials.effective_q_api_region(config).to_string();
        let (raw_debug_enabled, raw_debug_max_chars) =
            raw_debug_config_for_model(settings, model_for_metrics);
        let raw_request_id = raw_debug_enabled.then(|| Uuid::new_v4().to_string());

        Ok(ApiRequestParts {
            endpoint,
            endpoint_name,
            url,
            body,
            api_region,
            raw_request_id,
            raw_debug_enabled,
            raw_debug_max_chars,
        })
    }

    fn apply_message_pruning(
        parts: &mut ApiRequestParts,
        settings: &crate::kiro::settings::RuntimeSettings,
        request_log: Option<&RequestLogContext>,
        external_model: &str,
        credential_id: u64,
        is_stream: bool,
    ) {
        let config = MessagePruningConfig::from(settings);
        match guard_kiro_payload(&parts.body, &config) {
            MessagePruningOutcome::Noop => {}
            MessagePruningOutcome::Skipped(stats) => {
                Self::log_message_pruning_skipped(
                    &stats,
                    &config,
                    request_log,
                    external_model,
                    credential_id,
                    parts.endpoint_name.as_str(),
                    parts.api_region.as_str(),
                    is_stream,
                );
            }
            MessagePruningOutcome::Pruned { body, stats } => {
                parts.replace_body(body);
                Self::log_message_pruned(
                    &stats,
                    &config,
                    request_log,
                    external_model,
                    credential_id,
                    parts.endpoint_name.as_str(),
                    parts.api_region.as_str(),
                    is_stream,
                );
            }
        }
    }

    async fn send_api_attempt(
        &self,
        ctx: CallContext,
        parts: &ApiRequestParts,
        turbo_attempt_index: Option<usize>,
    ) -> ApiSendOutcome {
        let config = self.token_manager.config();
        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);
        let rctx = RequestContext {
            credentials: &ctx.credentials,
            token: &ctx.token,
            machine_id: &machine_id,
            config,
        };
        let base = match self.client_for(ctx.id, &ctx.credentials) {
            Ok(client) => client
                .post(&parts.url)
                .body(parts.body.clone())
                .header("content-type", "application/json"),
            Err(error) => {
                return ApiSendOutcome::Failure(ApiSendFailure {
                    ctx,
                    status: None,
                    body: String::new(),
                    upstream_error: UpstreamErrorInfo {
                        reason: None,
                        retry_after_ms: None,
                    },
                    error: Some(error),
                    upstream_ms: 0,
                    turbo_attempt_index,
                });
            }
        };
        let request = parts.endpoint.decorate_api(base, &rctx);
        let upstream_started_at = Instant::now();
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let upstream_ms = duration_ms(upstream_started_at.elapsed());
                if is_proxy_error(&error) {
                    self.mark_dynamic_proxy_failure(ctx.id, &error).await;
                }
                return ApiSendOutcome::Failure(ApiSendFailure {
                    ctx,
                    status: None,
                    body: String::new(),
                    upstream_error: UpstreamErrorInfo {
                        reason: None,
                        retry_after_ms: None,
                    },
                    error: Some(error.into()),
                    upstream_ms,
                    turbo_attempt_index,
                });
            }
        };

        let status = response.status();
        let status_u16 = status.as_u16();
        let retry_after_ms = parse_retry_after_ms(response.headers());
        let mut upstream_ms = duration_ms(upstream_started_at.elapsed());
        if status.is_success() {
            return ApiSendOutcome::Success(ApiSendSuccess {
                response,
                ctx,
                status: status_u16,
                upstream_ms,
                turbo_attempt_index,
            });
        }

        let body_started_at = Instant::now();
        let body = response.text().await.unwrap_or_default();
        upstream_ms = upstream_ms.saturating_add(duration_ms(body_started_at.elapsed()));
        ApiSendOutcome::Failure(ApiSendFailure {
            ctx,
            status: Some(status_u16),
            upstream_error: UpstreamErrorInfo {
                reason: extract_upstream_reason(&body),
                retry_after_ms,
            },
            body,
            error: None,
            upstream_ms,
            turbo_attempt_index,
        })
    }

    async fn send_api_with_policy(
        &self,
        ctx: CallContext,
        parts: &ApiRequestParts,
        policy: Option<CredentialPolicy>,
        model: Option<&str>,
        model_for_metrics: &str,
        is_stream: bool,
        request_log: Option<&RequestLogContext>,
    ) -> ApiSendBatch {
        let policy = policy.unwrap_or_else(CredentialPolicy::default);
        let requested_fanout = policy.effective_turbo_fanout();
        if policy.effective_turbo_mode() != "race" || requested_fanout <= 1 {
            let outcome = self.send_api_attempt(ctx, parts, None).await;
            let upstream_ms = match &outcome {
                ApiSendOutcome::Success(success) => success.upstream_ms,
                ApiSendOutcome::Failure(failure) => failure.upstream_ms,
            };
            return ApiSendBatch {
                outcome,
                actual_fanout: 1,
                upstream_ms,
            };
        }

        let credential_id = ctx.id;
        let mut contexts = vec![ctx];
        let additional = self
            .token_manager
            .acquire_additional_contexts_for_credential(
                credential_id,
                model,
                requested_fanout.saturating_sub(1),
            )
            .await;
        contexts.extend(additional);
        let actual_fanout = contexts.len();
        let request_id = request_log.map_or("", |log| log.request_id.as_str());
        let route = request_log.map_or("", |log| log.route);
        tracing::info!(
            request_id,
            route,
            model = model_for_metrics,
            credential_id,
            turbo_mode = policy.effective_turbo_mode(),
            requested_fanout,
            actual_fanout,
            stream = is_stream,
            "kiro_api_turbo_start"
        );

        let turbo_started_at = Instant::now();
        if actual_fanout <= 1 {
            let only = contexts
                .pop()
                .expect("Turbo context vector must contain initial context");
            let outcome = self.send_api_attempt(only, parts, Some(0)).await;
            let upstream_ms = match &outcome {
                ApiSendOutcome::Success(success) => success.upstream_ms,
                ApiSendOutcome::Failure(failure) => failure.upstream_ms,
            };
            match &outcome {
                ApiSendOutcome::Success(success) => {
                    tracing::info!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = success.ctx.id,
                        turbo_attempt_index = 0usize,
                        status = success.status,
                        latency_ms = success.upstream_ms,
                        winner = true,
                        "kiro_api_turbo_attempt_success"
                    );
                    tracing::info!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = success.ctx.id,
                        winner_attempt_index = 0usize,
                        wasted_attempts = 0usize,
                        header_ms = success.upstream_ms,
                        total_ms = duration_ms(turbo_started_at.elapsed()),
                        "kiro_api_turbo_winner"
                    );
                }
                ApiSendOutcome::Failure(failure) => {
                    let status_for_log = failure.status.unwrap_or(0);
                    tracing::warn!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = failure.ctx.id,
                        turbo_attempt_index = 0usize,
                        status = status_for_log,
                        latency_ms = failure.upstream_ms,
                        upstream_reason = failure.upstream_error.reason.as_deref(),
                        winner = false,
                        error = failure
                            .error
                            .as_ref()
                            .map(|err| err.to_string())
                            .unwrap_or_default(),
                        "kiro_api_turbo_attempt_failure"
                    );
                    tracing::warn!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = failure.ctx.id,
                        actual_fanout,
                        statuses = ?vec![status_for_log.to_string()],
                        "kiro_api_turbo_all_failed"
                    );
                }
            }
            return ApiSendBatch {
                outcome,
                actual_fanout,
                upstream_ms,
            };
        }

        let mut attempts = futures::stream::FuturesUnordered::new();
        for (idx, attempt_ctx) in contexts.into_iter().enumerate() {
            attempts.push(self.send_api_attempt(attempt_ctx, parts, Some(idx)));
        }

        let mut failures = Vec::new();
        while let Some(outcome) = attempts.next().await {
            match outcome {
                ApiSendOutcome::Success(success) => {
                    let winner_attempt_index = success.turbo_attempt_index.unwrap_or(0);
                    tracing::info!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = success.ctx.id,
                        turbo_attempt_index = winner_attempt_index,
                        status = success.status,
                        latency_ms = success.upstream_ms,
                        winner = true,
                        "kiro_api_turbo_attempt_success"
                    );
                    tracing::info!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = success.ctx.id,
                        winner_attempt_index,
                        wasted_attempts = actual_fanout.saturating_sub(1),
                        header_ms = success.upstream_ms,
                        total_ms = duration_ms(turbo_started_at.elapsed()),
                        "kiro_api_turbo_winner"
                    );
                    return ApiSendBatch {
                        upstream_ms: success.upstream_ms,
                        outcome: ApiSendOutcome::Success(success),
                        actual_fanout,
                    };
                }
                ApiSendOutcome::Failure(failure) => {
                    let attempt_index = failure.turbo_attempt_index.unwrap_or(0);
                    let status_for_log = failure.status.unwrap_or(0);
                    tracing::warn!(
                        request_id,
                        route,
                        model = model_for_metrics,
                        credential_id = failure.ctx.id,
                        turbo_attempt_index = attempt_index,
                        status = status_for_log,
                        latency_ms = failure.upstream_ms,
                        upstream_reason = failure.upstream_error.reason.as_deref(),
                        winner = false,
                        error = failure
                            .error
                            .as_ref()
                            .map(|err| err.to_string())
                            .unwrap_or_default(),
                        "kiro_api_turbo_attempt_failure"
                    );
                    failures.push(failure);
                }
            }
        }

        let upstream_ms = duration_ms(turbo_started_at.elapsed());
        let statuses: Vec<String> = failures
            .iter()
            .map(|failure| {
                failure
                    .status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "send_error".to_string())
            })
            .collect();
        tracing::warn!(
            request_id,
            route,
            model = model_for_metrics,
            credential_id,
            actual_fanout,
            statuses = ?statuses,
            "kiro_api_turbo_all_failed"
        );

        let representative = failures
            .into_iter()
            .max_by_key(|failure| turbo_failure_priority(failure.status))
            .expect("Turbo all-failed path must have at least one failure");
        ApiSendBatch {
            outcome: ApiSendOutcome::Failure(representative),
            actual_fanout,
            upstream_ms,
        }
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）
    #[allow(dead_code)]
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, false, None, 0, None, None)
            .await
    }

    pub async fn call_api_with_session_and_dump(
        &self,
        request_body: &str,
        session_id: Option<&str>,
        queue_ms: u64,
        prompt_dump: Option<PromptDump>,
        request_log: Option<RequestLogContext>,
    ) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(
            request_body,
            false,
            session_id,
            queue_ms,
            prompt_dump,
            request_log,
        )
        .await
    }

    /// 发送流式 API 请求
    #[allow(dead_code)]
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(request_body, true, None, 0, None, None)
            .await
    }

    pub async fn call_api_stream_with_session_and_dump(
        &self,
        request_body: &str,
        session_id: Option<&str>,
        queue_ms: u64,
        prompt_dump: Option<PromptDump>,
        request_log: Option<RequestLogContext>,
    ) -> anyhow::Result<LeasedResponse> {
        self.call_api_with_retry(
            request_body,
            true,
            session_id,
            queue_ms,
            prompt_dump,
            request_log,
        )
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

    pub async fn test_credential_message(
        &self,
        credential_id: u64,
        model: &str,
        prompt: &str,
    ) -> anyhow::Result<CredentialTestResult> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            anyhow::bail!("测试消息不能为空");
        }
        let mapped_model = map_admin_test_model(model)
            .ok_or_else(|| anyhow::anyhow!("不支持的测试模型: {}", model))?;
        let conversation_id = format!("admin-test-{}", Uuid::new_v4());
        let request = KiroRequest {
            conversation_state: ConversationState::new(conversation_id)
                .with_agent_task_type("vibe")
                .with_chat_trigger_type("MANUAL")
                .with_current_message(CurrentMessage::new(UserInputMessage::new(
                    prompt,
                    &mapped_model,
                ))),
            profile_arn: None,
        };
        let request_body = serde_json::to_string(&request)?;
        let started_at = Instant::now();
        let fixed = self
            .call_api_with_fixed_credential(&request_body, credential_id, &mapped_model)
            .await?;
        let status = fixed.status;
        let endpoint = fixed.endpoint;
        let api_region = fixed.api_region;
        let response_bytes = fixed.response.bytes().await?;
        let response_text = parse_assistant_response_text(&response_bytes);
        Ok(CredentialTestResult {
            credential_id,
            model: mapped_model,
            prompt: prompt.to_string(),
            response_text,
            status,
            latency_ms: duration_ms(started_at.elapsed()),
            endpoint,
            api_region,
        })
    }

    async fn call_api_with_fixed_credential(
        &self,
        request_body: &str,
        credential_id: u64,
        model: &str,
    ) -> anyhow::Result<FixedCredentialApiResponse> {
        let ctx = self
            .token_manager
            .acquire_context_for_credential(credential_id, Some(model))
            .await?;
        let config = self.token_manager.config();
        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);
        let endpoint = self.endpoint_for(&ctx.credentials)?;
        let rctx = RequestContext {
            credentials: &ctx.credentials,
            token: &ctx.token,
            machine_id: &machine_id,
            config,
        };
        let url = endpoint.api_url(&rctx);
        let body = endpoint.transform_api_body(request_body, &rctx);
        let api_region = rctx.credentials.effective_q_api_region(config).to_string();
        let endpoint_name = endpoint.name().to_string();

        tracing::info!(
            credential_id = ctx.id,
            model,
            endpoint = endpoint.name(),
            api_region = api_region.as_str(),
            url = %url,
            "admin_credential_test_request"
        );

        let base = self
            .client_for(ctx.id, &ctx.credentials)?
            .post(&url)
            .body(body)
            .header("content-type", "application/json");
        let request = endpoint.decorate_api(base, &rctx);
        let response = match request.send().await {
            Ok(resp) => resp,
            Err(err) => {
                if is_proxy_error(&err) {
                    self.mark_dynamic_proxy_failure(ctx.id, &err).await;
                }
                self.token_manager.report_transient_error(ctx.id);
                return Err(err.into());
            }
        };
        let status = response.status();
        if status.is_success() {
            self.token_manager.report_success(ctx.id);
            return Ok(FixedCredentialApiResponse {
                response: LeasedResponse::new(
                    response, ctx.id, 1, None, false, 0, None, ctx.lease, None,
                ),
                endpoint: endpoint_name,
                api_region,
                status: status.as_u16(),
            });
        }

        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
            self.token_manager.report_quota_exhausted(ctx.id);
        } else if status.as_u16() == 402 && endpoint.is_overage_limit(&body) {
            self.token_manager.stop_overage_for(ctx.id);
        } else if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
            if status.as_u16() == 429 {
                self.token_manager.report_rate_limited(ctx.id);
            } else {
                self.token_manager.report_transient_error(ctx.id);
            }
        } else if matches!(status.as_u16(), 401 | 403) {
            self.token_manager.report_failure(ctx.id);
        }
        anyhow::bail!("账号测试失败: {} {}", status, body);
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
            tracing::info!(
                credential_id = ctx.id,
                endpoint = endpoint.name(),
                api_region = rctx.credentials.effective_q_api_region(config),
                url = %url,
                "kiro_mcp_request_endpoint"
            );

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
                    None,
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

            if status.as_u16() == 402 && endpoint.is_overage_limit(&body) {
                self.token_manager.stop_overage_for(ctx.id);
                last_error = Some(anyhow::anyhow!(
                    "MCP 请求透支被上游拒绝: {} {}",
                    status,
                    body
                ));
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
        prompt_dump: Option<PromptDump>,
        request_log: Option<RequestLogContext>,
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
        let mut model_capacity_failures_by_credential: HashMap<u64, usize> = HashMap::new();
        let mut same_account_retry_counts: HashMap<String, usize> = HashMap::new();
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
                request_log: request_log.clone(),
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
        let mut upstream_ms_total: u64 = 0;
        let mut retry_same_credential: Option<u64> = None;

        loop {
            if attempted_credentials.len() >= max_retry_accounts {
                break;
            }

            // 获取调用上下文（绑定 index、credentials、token）
            let acquire_started_at = Instant::now();
            let ctx = match retry_same_credential.take() {
                Some(credential_id) => {
                    match self
                        .token_manager
                        .acquire_context_for_credential(credential_id, model.as_deref())
                        .await
                    {
                        Ok(c) => {
                            acquire_ms_total += duration_ms(acquire_started_at.elapsed());
                            c
                        }
                        Err(e) => {
                            acquire_ms_total += duration_ms(acquire_started_at.elapsed());
                            tracing::warn!(
                                credential_id,
                                error = %e,
                                "同号重试获取凭据失败，切换到账号故障转移"
                            );
                            excluded_credentials.insert(credential_id);
                            last_error = Some(e);
                            continue;
                        }
                    }
                }
                None => match self
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
                },
            };
            if !attempted_credentials.contains(&ctx.id) {
                attempted_credentials.push(ctx.id);
            }
            last_credential_id = Some(ctx.id);

            let config = self.token_manager.config();
            let mut parts = match self.build_api_request_parts(
                &ctx,
                request_body,
                is_stream,
                &model_for_metrics,
                &settings,
            ) {
                Ok(parts) => parts,
                Err(e) => {
                    last_error = Some(e);
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };
            let request_log_ref = request_log.as_ref();
            tracing::info!(
                request_id = request_log_ref.map_or("", |log| log.request_id.as_str()),
                route = request_log_ref.map_or("", |log| log.route),
                client_device_id = request_log_ref.map_or("", RequestLogContext::client_device_id_for_log),
                client_account_uuid = request_log_ref.map_or("", RequestLogContext::client_account_uuid_for_log),
                client_user = request_log_ref.map_or("", RequestLogContext::client_user_for_log),
                client_session_id = request_log_ref.map_or("", RequestLogContext::client_session_id_for_log),
                usage_session_key = request_log_ref.map_or("", |log| log.usage_session_key.as_str()),
                model = model_for_metrics.as_str(),
                credential_id = ctx.id,
                endpoint = parts.endpoint_name.as_str(),
                api_region = parts.api_region.as_str(),
                stream = is_stream,
                url = %parts.url,
                "kiro_api_request_endpoint"
            );
            Self::apply_message_pruning(
                &mut parts,
                &settings,
                request_log_ref,
                model_for_metrics.as_str(),
                ctx.id,
                is_stream,
            );
            if let Some(raw_request_id) = parts.raw_request_id.as_deref() {
                log_kiro_raw_request(
                    raw_request_id,
                    model_for_metrics.as_str(),
                    ctx.id,
                    parts.endpoint_name.as_str(),
                    parts.url.as_str(),
                    is_stream,
                    &parts.body,
                    parts.raw_debug_max_chars,
                );
            }
            if let Some(dump) = prompt_dump.as_ref() {
                dump.write_text("upstream_request.json", &parts.body);
            }
            let request_diagnostics = if config.request_diagnostics_enabled {
                let diagnostics = Self::diagnose_api_request_body(&parts.body);
                Self::log_request_diagnostics(
                    &diagnostics,
                    request_log_ref,
                    model_for_metrics.as_str(),
                    ctx.id,
                    parts.endpoint_name.as_str(),
                    parts.api_region.as_str(),
                    config.kiro_version.as_str(),
                    config.node_version.as_str(),
                    config.system_version.as_str(),
                    is_stream,
                    "enabled",
                    None,
                    None,
                    None,
                );
                Some(diagnostics)
            } else {
                None
            };

            let credential_id = ctx.id;
            let policy = self.token_manager.policy_for_credential(credential_id);
            let send_batch = self
                .send_api_with_policy(
                    ctx,
                    &parts,
                    policy,
                    model.as_deref(),
                    &model_for_metrics,
                    is_stream,
                    request_log_ref,
                )
                .await;
            http_attempts += send_batch.actual_fanout;
            upstream_ms_total = upstream_ms_total.saturating_add(send_batch.upstream_ms);

            let (ctx, status, body, upstream_error) = match send_batch.outcome {
                ApiSendOutcome::Success(success) => {
                    self.token_manager.report_success(success.ctx.id);
                    let status = reqwest::StatusCode::from_u16(success.status)
                        .unwrap_or(reqwest::StatusCode::OK);
                    let mut timing = ResponseTimingGuard::new(
                        self.metrics.clone(),
                        ApiTimingRecord {
                            model: model_for_metrics.clone(),
                            is_stream,
                            credential_id: Some(success.ctx.id),
                            status: Some(status.as_u16()),
                            outcome: UpstreamOutcome::Success,
                            attempts: http_attempts,
                            queue_ms,
                            acquire_ms: acquire_ms_total,
                            upstream_ms: upstream_ms_total,
                            total_ms: 0,
                            request_log: request_log.clone(),
                        },
                        total_started_at,
                        Instant::now(),
                    );
                    if is_stream {
                        timing = timing.with_stream_log(StreamTimingLogContext {
                            model: model_for_metrics.clone(),
                            credential_id: success.ctx.id,
                            status: status.as_u16(),
                            attempts: http_attempts,
                            queue_ms,
                            acquire_ms: acquire_ms_total,
                            header_ms: upstream_ms_total,
                            total_started_at,
                            request_log: request_log.clone(),
                        });
                    }
                    return Ok(LeasedResponse::new(
                        success.response,
                        success.ctx.id,
                        http_attempts,
                        parts.raw_request_id.clone(),
                        parts.raw_debug_enabled,
                        parts.raw_debug_max_chars,
                        prompt_dump.clone(),
                        success.ctx.lease,
                        Some(timing),
                    ));
                }
                ApiSendOutcome::Failure(failure) if failure.status.is_none() => {
                    let mut fallback_diagnostics = None;
                    let diagnostics = Self::request_diagnostics_for_log(
                        &request_diagnostics,
                        &mut fallback_diagnostics,
                        &parts.body,
                    );
                    Self::log_request_diagnostics(
                        diagnostics,
                        request_log_ref,
                        model_for_metrics.as_str(),
                        failure.ctx.id,
                        parts.endpoint_name.as_str(),
                        parts.api_region.as_str(),
                        config.kiro_version.as_str(),
                        config.node_version.as_str(),
                        config.system_version.as_str(),
                        is_stream,
                        "upstream_failure",
                        None,
                        failure.upstream_error.reason.as_deref(),
                        failure.upstream_error.retry_after_ms,
                    );
                    tracing::warn!(
                        credential_id = failure.ctx.id,
                        attempted_credentials = ?attempted_credentials,
                        excluded_credential_ids = ?excluded_credentials,
                        turbo_attempt_index = failure.turbo_attempt_index,
                        "API 请求发送失败（尝试账号 {}/{}）: {}",
                        attempted_credentials.len(),
                        max_retry_accounts,
                        failure
                            .error
                            .as_ref()
                            .map(|err| err.to_string())
                            .unwrap_or_else(|| "unknown send error".to_string())
                    );
                    self.token_manager
                        .report_transient_error_for(failure.ctx.id, settings.transient_cooldown_ms);
                    excluded_credentials.insert(failure.ctx.id);
                    last_error = Some(
                        failure
                            .error
                            .unwrap_or_else(|| anyhow::anyhow!("API 请求发送失败")),
                    );
                    if attempted_credentials.len() < max_retry_accounts {
                        sleep(Self::retry_delay(http_attempts)).await;
                    }
                    continue;
                }
                ApiSendOutcome::Failure(failure) => {
                    let status = reqwest::StatusCode::from_u16(failure.status.unwrap_or(500))
                        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                    last_status = Some(status.as_u16());
                    if let Some(dump) = prompt_dump.as_ref() {
                        dump.write_text("upstream_response.raw", &failure.body);
                        dump.update_meta(PromptDumpMetaUpdate {
                            route: "/v1/messages".to_string(),
                            model: model_for_metrics.clone(),
                            stream: is_stream,
                            credential_id: Some(failure.ctx.id),
                            attempts: Some(http_attempts),
                            status: Some(status.as_u16()),
                            duration_ms: Some(duration_ms(total_started_at.elapsed())),
                            signature_classification: None,
                            request_kind: None,
                            expected_text_only: None,
                            truncated: false,
                        });
                    }
                    last_retry_after_ms = failure
                        .upstream_error
                        .retry_after_ms
                        .or(last_retry_after_ms);
                    let mut fallback_diagnostics = None;
                    let diagnostics = Self::request_diagnostics_for_log(
                        &request_diagnostics,
                        &mut fallback_diagnostics,
                        &parts.body,
                    );
                    Self::log_request_diagnostics(
                        diagnostics,
                        request_log_ref,
                        model_for_metrics.as_str(),
                        failure.ctx.id,
                        parts.endpoint_name.as_str(),
                        parts.api_region.as_str(),
                        config.kiro_version.as_str(),
                        config.node_version.as_str(),
                        config.system_version.as_str(),
                        is_stream,
                        "upstream_failure",
                        Some(status.as_u16()),
                        failure.upstream_error.reason.as_deref(),
                        failure.upstream_error.retry_after_ms,
                    );
                    (failure.ctx, status, failure.body, failure.upstream_error)
                }
            };

            if let Some(rule) = matching_same_account_retry_rule(
                &settings.same_account_retry_rules,
                status.as_u16(),
                upstream_error.reason.as_deref(),
            ) {
                let retry_key = same_account_retry_key(
                    ctx.id,
                    status.as_u16(),
                    upstream_error.reason.as_deref(),
                    rule,
                );
                let retry_count = same_account_retry_counts.entry(retry_key).or_default();
                if *retry_count < rule.attempts {
                    *retry_count += 1;
                    let delay_ms = if rule.respect_retry_after {
                        upstream_error.retry_after_ms.unwrap_or(rule.delay_ms)
                    } else {
                        rule.delay_ms
                    };
                    tracing::info!(
                        request_id = request_log_ref.map_or("", |log| log.request_id.as_str()),
                        route = request_log_ref.map_or("", |log| log.route),
                        client_device_id =
                            request_log_ref.map_or("", RequestLogContext::client_device_id_for_log),
                        client_account_uuid = request_log_ref
                            .map_or("", RequestLogContext::client_account_uuid_for_log),
                        client_user =
                            request_log_ref.map_or("", RequestLogContext::client_user_for_log),
                        client_session_id = request_log_ref
                            .map_or("", RequestLogContext::client_session_id_for_log),
                        usage_session_key =
                            request_log_ref.map_or("", |log| log.usage_session_key.as_str()),
                        credential_id = ctx.id,
                        status = status.as_u16(),
                        upstream_reason = upstream_error.reason.as_deref(),
                        same_account_retry = *retry_count,
                        same_account_retry_limit = rule.attempts,
                        retry_after_ms = delay_ms,
                        rule_status = rule.status.as_str(),
                        "请求失败命中单号重试规则，继续使用同一账号重试"
                    );
                    retry_same_credential = Some(ctx.id);
                    sleep(Duration::from_millis(delay_ms)).await;
                    continue;
                }
            }

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && parts.endpoint.is_monthly_request_limit(&body) {
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
                        request_log: request_log.clone(),
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

            if status.as_u16() == 402 && parts.endpoint.is_overage_limit(&body) {
                tracing::warn!(
                    credential_id = ctx.id,
                    attempted_credentials = ?attempted_credentials,
                    excluded_credential_ids = ?excluded_credentials,
                    "API 请求透支被上游拒绝，关闭账号级透支并切换，尝试账号 {}/{}: {} {}",
                    attempted_credentials.len(),
                    max_retry_accounts,
                    status,
                    body
                );
                self.token_manager.stop_overage_for(ctx.id);
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求透支被上游拒绝: {} {}",
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
                    request_log: request_log.clone(),
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
                if parts.endpoint.is_bearer_token_invalid(&body)
                    && !force_refreshed.contains(&ctx.id)
                {
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
                        request_log: request_log.clone(),
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
                    *model_capacity_failures_by_credential
                        .entry(ctx.id)
                        .or_default() += 1;
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
                    request_log: request_log.clone(),
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

        let all_attempted_accounts_hit_model_capacity = !attempted_credentials.is_empty()
            && attempted_credentials.iter().all(|credential_id| {
                model_capacity_failures_by_credential.contains_key(credential_id)
            });
        if model_capacity_failures > 0 && all_attempted_accounts_hit_model_capacity {
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
            request_log: request_log.clone(),
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

    fn log_request_diagnostics(
        diagnostics: &RequestDiagnostics,
        request_log: Option<&RequestLogContext>,
        external_model: &str,
        credential_id: u64,
        endpoint: &str,
        api_region: &str,
        kiro_version: &str,
        node_version: &str,
        system_version: &str,
        stream: bool,
        diagnostics_kind: &'static str,
        upstream_status: Option<u16>,
        upstream_reason: Option<&str>,
        retry_after_ms: Option<u64>,
    ) {
        tracing::info!(
            request_id = request_log.map_or("", |log| log.request_id.as_str()),
            route = request_log.map_or("", |log| log.route),
            client_device_id = request_log.map_or("", RequestLogContext::client_device_id_for_log),
            client_account_uuid =
                request_log.map_or("", RequestLogContext::client_account_uuid_for_log),
            client_user = request_log.map_or("", RequestLogContext::client_user_for_log),
            client_session_id =
                request_log.map_or("", RequestLogContext::client_session_id_for_log),
            usage_session_key = request_log.map_or("", |log| log.usage_session_key.as_str()),
            model = diagnostics.model.as_deref().unwrap_or("unknown"),
            external_model,
            credential_id,
            endpoint,
            api_region,
            kiro_version,
            node_version,
            system_version,
            stream,
            conversation_id = diagnostics.conversation_id.as_deref(),
            agent_task_type = diagnostics.agent_task_type.as_deref(),
            chat_trigger_type = diagnostics.chat_trigger_type.as_deref(),
            request_bytes = diagnostics.request_bytes,
            history_len = diagnostics.history_len,
            history_user_count = diagnostics.history_user_count,
            history_assistant_count = diagnostics.history_assistant_count,
            history_bytes = diagnostics.history_bytes,
            largest_history_entry_bytes = diagnostics.largest_history_entry_bytes,
            current_content_chars = diagnostics.current_content_chars,
            current_message_bytes = diagnostics.current_message_bytes,
            tools_count = diagnostics.tools_count,
            tools_bytes = diagnostics.tools_bytes,
            largest_tool_bytes = diagnostics.largest_tool_bytes,
            tool_results_count = diagnostics.tool_results_count,
            tool_results_bytes = diagnostics.tool_results_bytes,
            largest_tool_result_bytes = diagnostics.largest_tool_result_bytes,
            current_images_count = diagnostics.current_images_count,
            profile_arn_present = diagnostics.profile_arn_present,
            thinking_directives_present = diagnostics.thinking_directives_present,
            diagnostics_kind,
            upstream_status,
            upstream_reason,
            retry_after_ms,
            "kiro_api_request_diagnostics"
        );
    }

    fn log_message_pruning_skipped(
        stats: &MessagePruningStats,
        config: &MessagePruningConfig,
        request_log: Option<&RequestLogContext>,
        external_model: &str,
        credential_id: u64,
        endpoint: &str,
        api_region: &str,
        stream: bool,
    ) {
        tracing::info!(
            request_id = request_log.map_or("", |log| log.request_id.as_str()),
            route = request_log.map_or("", |log| log.route),
            client_device_id = request_log.map_or("", RequestLogContext::client_device_id_for_log),
            client_account_uuid =
                request_log.map_or("", RequestLogContext::client_account_uuid_for_log),
            client_user = request_log.map_or("", RequestLogContext::client_user_for_log),
            client_session_id =
                request_log.map_or("", RequestLogContext::client_session_id_for_log),
            usage_session_key = request_log.map_or("", |log| log.usage_session_key.as_str()),
            external_model,
            credential_id,
            endpoint,
            api_region,
            stream,
            enabled = config.enabled,
            max_request_bytes = config.max_request_bytes,
            keep_recent_messages = config.keep_recent_messages,
            original_bytes = stats.original_bytes,
            final_bytes = stats.final_bytes,
            original_history_len = stats.original_history_len,
            final_history_len = stats.final_history_len,
            under_limit = stats.under_limit,
            "kiro_api_message_pruning_skipped"
        );
    }

    fn log_message_pruned(
        stats: &MessagePruningStats,
        config: &MessagePruningConfig,
        request_log: Option<&RequestLogContext>,
        external_model: &str,
        credential_id: u64,
        endpoint: &str,
        api_region: &str,
        stream: bool,
    ) {
        tracing::info!(
            request_id = request_log.map_or("", |log| log.request_id.as_str()),
            route = request_log.map_or("", |log| log.route),
            client_device_id = request_log.map_or("", RequestLogContext::client_device_id_for_log),
            client_account_uuid =
                request_log.map_or("", RequestLogContext::client_account_uuid_for_log),
            client_user = request_log.map_or("", RequestLogContext::client_user_for_log),
            client_session_id =
                request_log.map_or("", RequestLogContext::client_session_id_for_log),
            usage_session_key = request_log.map_or("", |log| log.usage_session_key.as_str()),
            external_model,
            credential_id,
            endpoint,
            api_region,
            stream,
            enabled = config.enabled,
            max_request_bytes = config.max_request_bytes,
            keep_recent_messages = config.keep_recent_messages,
            max_history_entry_bytes = config.max_history_entry_bytes,
            max_truncated_content_bytes = config.max_truncated_content_bytes,
            original_bytes = stats.original_bytes,
            final_bytes = stats.final_bytes,
            original_history_len = stats.original_history_len,
            final_history_len = stats.final_history_len,
            removed_entries = stats.removed_entries,
            truncated_entries = stats.truncated_entries,
            orphaned_tool_results_removed = stats.orphaned_tool_results_removed,
            empty_tool_uses_stripped = stats.empty_tool_uses_stripped,
            aligned_leading_entries_removed = stats.aligned_leading_entries_removed,
            under_limit = stats.under_limit,
            "kiro_api_message_pruned"
        );
    }

    fn request_diagnostics_for_log<'a>(
        cached: &'a Option<RequestDiagnostics>,
        fallback: &'a mut Option<RequestDiagnostics>,
        request_body: &str,
    ) -> &'a RequestDiagnostics {
        if let Some(diagnostics) = cached.as_ref() {
            return diagnostics;
        }
        fallback.get_or_insert_with(|| Self::diagnose_api_request_body(request_body))
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
                let entry_bytes = item.to_string().len();
                diagnostics.history_bytes += entry_bytes;
                diagnostics.largest_history_entry_bytes =
                    diagnostics.largest_history_entry_bytes.max(entry_bytes);
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
        diagnostics.current_message_bytes = user_input.to_string().len();

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
            if let Some(tools) = context.get("tools").and_then(Value::as_array) {
                diagnostics.tools_count = tools.len();
                for tool in tools {
                    let tool_bytes = tool.to_string().len();
                    diagnostics.tools_bytes += tool_bytes;
                    diagnostics.largest_tool_bytes = diagnostics.largest_tool_bytes.max(tool_bytes);
                }
            }
            if let Some(tool_results) = context.get("toolResults").and_then(Value::as_array) {
                diagnostics.tool_results_count = tool_results.len();
                for result in tool_results {
                    let result_bytes = result.to_string().len();
                    diagnostics.tool_results_bytes += result_bytes;
                    diagnostics.largest_tool_result_bytes =
                        diagnostics.largest_tool_result_bytes.max(result_bytes);
                }
            }
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
        assert!(summary.current_message_bytes > 0);
        assert!(summary.history_bytes > 0);
        assert!(summary.largest_history_entry_bytes > 0);
        assert!(summary.tools_bytes > 0);
        assert!(summary.largest_tool_bytes > 0);
        assert!(summary.tool_results_bytes > 0);
        assert!(summary.largest_tool_result_bytes > 0);
        assert!(summary.profile_arn_present);
        assert!(summary.thinking_directives_present);
        assert!(summary.request_bytes > 0);
    }
}
