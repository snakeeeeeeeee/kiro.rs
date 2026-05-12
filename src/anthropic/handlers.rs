//! Anthropic API Handler 函数

use std::{borrow::Cow, convert::Infallible, sync::Arc, time::Instant};

use crate::kiro::model::events::Event;
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::provider::ProviderRateLimitError;
use crate::runtime::GlobalRequestPermit;
use crate::token;
use anyhow::Error;
use axum::{
    Json as JsonExtractor,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use serde_json::json;
use std::time::Duration;
use tokio::time::interval;
use uuid::Uuid;

use super::converter::{ConversionError, convert_request};
use super::middleware::AppState;
use super::stream::{BufferedStreamContext, Opus47Diagnostics, SseEvent, StreamContext};
use super::types::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model, ModelsResponse,
    OutputConfig, Thinking,
};
use super::usage::{
    AnthropicUsage, CacheTtl, VirtualCacheUsageManager, VirtualUsageInput,
    estimate_latest_user_input_tokens, request_cache_ttl, session_key_for_request,
};
use super::websearch;

/// GET /healthz
pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// GET /readyz
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ready = state
        .kiro_provider
        .as_ref()
        .map(|provider| provider.token_manager().has_ready_credential())
        .unwrap_or(false);

    if ready {
        (
            StatusCode::OK,
            Json(json!({
                "status": "ready"
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "not_ready",
                "reason": "no dispatchable credential"
            })),
        )
    }
}

/// 将 KiroProvider 错误映射为 HTTP 响应
fn map_provider_error(err: Error) -> Response {
    let err_str = err.to_string();

    // 上下文窗口满了（对话历史累积超出模型上下文窗口限制）
    if err_str.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
        tracing::warn!(error = %err, "上游拒绝请求：上下文窗口已满（不应重试）");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                "Context window is full. Reduce conversation history, system prompt, or tools.",
            )),
        )
            .into_response();
    }

    // 单次输入太长（请求体本身超出上游限制）
    if err_str.contains("Input is too long") {
        tracing::warn!(error = %err, "上游拒绝请求：输入过长（不应重试）");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                "Input is too long. Reduce the size of your messages.",
            )),
        )
            .into_response();
    }

    if err_str.contains("没有可调度凭据") || err_str.contains("所有凭据均无法获取有效 Token")
    {
        tracing::warn!(error = %err, "当前没有可调度凭据");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse::new(
                "rate_limit_error",
                "当前没有可调度账号，请稍后重试",
            )),
        )
            .into_response();
    }

    if let Some(rate_limit) = err.downcast_ref::<ProviderRateLimitError>() {
        tracing::warn!(error = %err, retry_after_secs = rate_limit.retry_after_secs, "上游请求被限流");
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse::new(
                "rate_limit_error",
                rate_limit.message.clone(),
            )),
        )
            .into_response();
        if let Some(seconds) = rate_limit.retry_after_secs {
            if let Ok(value) = seconds.to_string().parse() {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        return response;
    }

    if err_str.contains("所有凭据均已禁用") || err_str.contains("所有凭据已用尽") {
        tracing::warn!(error = %err, "没有可用凭据");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse::new(
                "service_unavailable",
                "没有可用账号，请检查凭据状态",
            )),
        )
            .into_response();
    }

    tracing::error!("Kiro API 调用失败: {}", err);
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse::new(
            "api_error",
            format!("上游 API 调用失败: {}", err),
        )),
    )
        .into_response()
}

/// GET /v1/models
///
/// 返回可用的模型列表
pub async fn get_models() -> impl IntoResponse {
    tracing::info!("Received GET /v1/models request");

    let models = vec![
        Model {
            id: "claude-opus-4-7".to_string(),
            object: "model".to_string(),
            created: 1776297600, // Apr 16, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-7-thinking".to_string(),
            object: "model".to_string(),
            created: 1776297600, // Apr 16, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-5-20251101-thinking".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929-thinking".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001-thinking".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
    ];

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Message;
    use crate::kiro::model::events::{
        AssistantResponseEvent, Event, ReasoningContentEvent, ToolUseEvent,
    };
    use crate::kiro::settings::RuntimeSettings;
    use crate::model::config::Config;

    fn request(model: &str) -> MessagesRequest {
        MessagesRequest {
            model: model.to_string(),
            max_tokens: 1024,
            messages: vec![Message {
                role: "user".to_string(),
                content: serde_json::json!("test"),
            }],
            stream: true,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    fn settings(mode: &str) -> RuntimeSettings {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.opus47_plain_stabilization_mode = mode.to_string();
        settings
    }

    #[test]
    fn plain_opus47_mode_off_does_not_inject_adaptive() {
        let mut payload = request("claude-opus-4-7");
        let mode =
            apply_opus47_plain_stabilization(&mut payload, "claude-opus-4-7", &settings("off"));

        assert_eq!(mode, "off");
        assert!(payload.thinking.is_none());
        assert!(payload.output_config.is_none());
        assert!(!client_thinking_enabled_for_request(
            "claude-opus-4-7",
            &payload
        ));
    }

    #[test]
    fn plain_opus47_adaptive_low_injects_upstream_but_hides_client_thinking() {
        let mut payload = request("claude-opus-4-7");
        let mode = apply_opus47_plain_stabilization(
            &mut payload,
            "claude-opus-4-7",
            &settings("adaptive_low"),
        );

        assert_eq!(mode, "adaptive_low");
        assert_eq!(
            payload.thinking.as_ref().map(|t| t.thinking_type.as_str()),
            Some("adaptive")
        );
        assert_eq!(
            payload.output_config.as_ref().map(|c| c.effort.as_str()),
            Some("low")
        );
        assert!(!client_thinking_enabled_for_request(
            "claude-opus-4-7",
            &payload
        ));
    }

    #[test]
    fn opus47_thinking_keeps_adaptive_high_and_exposes_client_thinking() {
        let mut payload = request("claude-opus-4-7-thinking");
        override_thinking_from_model_name(&mut payload);
        let mode = apply_opus47_plain_stabilization(
            &mut payload,
            "claude-opus-4-7-thinking",
            &settings("adaptive_low"),
        );

        assert_eq!(mode, "off");
        assert_eq!(
            payload.thinking.as_ref().map(|t| t.thinking_type.as_str()),
            Some("adaptive")
        );
        assert_eq!(
            payload.output_config.as_ref().map(|c| c.effort.as_str()),
            Some("high")
        );
        assert!(client_thinking_enabled_for_request(
            "claude-opus-4-7-thinking",
            &payload
        ));
    }

    #[test]
    fn plain_opus47_reasoning_is_hidden_but_counted_in_diagnostics() {
        let mut ctx = StreamContext::new_with_thinking(
            "claude-opus-4-7",
            1,
            false,
            std::collections::HashMap::new(),
        );
        ctx.set_opus47_diagnostics(Opus47Diagnostics::new(
            true,
            "claude-opus-4-7",
            7,
            1,
            "adaptive_low",
            false,
        ));

        let events = ctx.process_kiro_event(&Event::ReasoningContent(ReasoningContentEvent {
            text: Some("hidden text".to_string()),
            signature: Some("sig".to_string()),
        }));

        assert!(events.is_empty());
        let diagnostics = ctx.opus47_diagnostics();
        assert_eq!(diagnostics.reasoning_content_count, 1);
        assert_eq!(
            diagnostics.hidden_reasoning_chars,
            "hidden text".chars().count()
        );
        assert!(diagnostics.signature_seen);
        assert_eq!(diagnostics.first_event_type(), "reasoning_content");
    }

    #[test]
    fn opus47_diagnostics_counts_visible_and_tool_events() {
        let mut diagnostics = Opus47Diagnostics::new(true, "claude-opus-4-7", 7, 2, "off", false);
        let mut assistant = AssistantResponseEvent::default();
        assistant.content = "hello".to_string();
        diagnostics.observe_event(&Event::AssistantResponse(assistant));
        diagnostics.observe_event(&Event::ToolUse(ToolUseEvent {
            name: "Read".to_string(),
            tool_use_id: "toolu_1".to_string(),
            input: "{}".to_string(),
            stop: true,
        }));

        assert_eq!(diagnostics.assistant_response_count, 1);
        assert_eq!(diagnostics.tool_use_count, 1);
        assert_eq!(diagnostics.visible_text_chars, 5);
        assert_eq!(diagnostics.first_event_type(), "assistant_response");
    }
}

/// POST /v1/messages
///
/// 创建消息（对话）
pub async fn post_messages(
    State(state): State<AppState>,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages request"
    );
    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    let requested_model = payload.model.clone();
    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);
    let runtime_settings = provider.token_manager().runtime_settings();
    let usage_session_key = session_key_for_request(
        &payload,
        &payload.model,
        &runtime_settings.virtual_cache_fallback_scope,
    );
    let request_ttl =
        request_cache_ttl(&payload, CacheTtl::from_runtime_default(&runtime_settings));
    let estimated_uncached_input_tokens = estimate_latest_user_input_tokens(&payload);

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        let permit = match state
            .runtime_limiter
            .acquire(provider.token_manager())
            .await
        {
            Ok(permit) => permit,
            Err(e) => return e.into_response(),
        };

        return websearch::handle_websearch_request(provider, &payload, input_tokens, permit).await;
    }

    let stabilization_mode =
        apply_opus47_plain_stabilization(&mut payload, &requested_model, &runtime_settings);
    let client_thinking_enabled = client_thinking_enabled_for_request(&requested_model, &payload);
    let opus47_diagnostics_enabled =
        runtime_settings.opus47_diagnostics_enabled && is_opus47_model_name(&requested_model);

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求（profile_arn 由 provider 层根据实际凭据注入）
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: None,
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;

    let session_affinity_key = conversion_result.session_affinity_key;
    let tool_name_map = conversion_result.tool_name_map;

    let _permit = match state
        .runtime_limiter
        .acquire(provider.token_manager())
        .await
    {
        Ok(permit) => permit,
        Err(e) => return e.into_response(),
    };

    if payload.stream {
        // 流式响应
        handle_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            estimated_uncached_input_tokens,
            client_thinking_enabled,
            stabilization_mode.as_str(),
            opus47_diagnostics_enabled,
            tool_name_map,
            Some(session_affinity_key.as_str()),
            usage_session_key,
            state.virtual_cache_usage.clone(),
            request_ttl,
            _permit,
        )
        .await
    } else {
        // 非流式响应：仅在配置开启时提取 thinking 块
        let extract_thinking = state.extract_thinking && client_thinking_enabled;
        handle_non_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            estimated_uncached_input_tokens,
            extract_thinking,
            client_thinking_enabled,
            stabilization_mode.as_str(),
            opus47_diagnostics_enabled,
            tool_name_map,
            Some(session_affinity_key.as_str()),
            usage_session_key,
            state.virtual_cache_usage.clone(),
            request_ttl,
            _permit,
        )
        .await
    }
}

/// 处理流式请求
async fn handle_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    estimated_uncached_input_tokens: i32,
    client_thinking_enabled: bool,
    stabilization_mode: &str,
    opus47_diagnostics_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    session_id: Option<&str>,
    usage_session_key: String,
    usage_manager: Arc<VirtualCacheUsageManager>,
    request_ttl: CacheTtl,
    permit: GlobalRequestPermit,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider
        .call_api_stream_with_session(request_body, session_id, permit.queue_ms())
        .await
    {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    // 创建流处理上下文
    let credential_id = response.credential_id();
    let attempts = response.attempts();
    let settings = provider.token_manager().runtime_settings();
    let pending_usage = usage_manager.preview_usage(
        &settings,
        VirtualUsageInput {
            credential_id,
            model: model.to_string(),
            session_key: usage_session_key,
            observed_total_input_tokens: input_tokens,
            estimated_uncached_input_tokens: Some(estimated_uncached_input_tokens),
            output_tokens: 1,
            creation_ttl: request_ttl,
        },
    );
    let initial_usage = pending_usage.usage().clone();

    let mut ctx = StreamContext::new_with_thinking(
        model,
        input_tokens,
        client_thinking_enabled,
        tool_name_map,
    );
    ctx.set_opus47_diagnostics(Opus47Diagnostics::new(
        opus47_diagnostics_enabled,
        model,
        credential_id,
        attempts,
        stabilization_mode,
        client_thinking_enabled,
    ));
    ctx.set_initial_usage(initial_usage);
    ctx.set_pending_usage_commit(usage_manager, pending_usage);

    // 生成初始事件
    let initial_events = ctx.generate_initial_events();

    // 创建 SSE 流
    let stream = create_sse_stream(response, ctx, initial_events, permit);

    // 返回 SSE 响应
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 创建 SSE 事件流
fn create_sse_stream(
    response: crate::kiro::provider::LeasedResponse,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
    permit: GlobalRequestPermit,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let credential_id = response.credential_id();
    let stream_model = ctx.model.clone();
    let stream_started_at = Instant::now();

    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
            Some(permit),
            stream_model,
            credential_id,
            stream_started_at,
        ),
        |(
            mut body_stream,
            mut ctx,
            mut decoder,
            finished,
            mut first_event_logged,
            mut ping_interval,
            permit,
            stream_model,
            credential_id,
            stream_started_at,
        )| async move {
            if finished {
                return None;
            }

            // 使用 select! 同时等待数据和 ping 定时器
            tokio::select! {
                // 处理数据流
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // 解码事件
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut events = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        if let Ok(event) = Event::from_frame(frame) {
                                            if !first_event_logged {
                                                first_event_logged = true;
                                                tracing::info!(
                                                    target: "kiro_rs::metrics",
                                                    model = %stream_model,
                                                    stream = true,
                                                    credential_id,
                                                    first_event_ms = crate::metrics::duration_ms(stream_started_at.elapsed()),
                                                    event_type = %event_metric_name(&event),
                                                    "upstream_stream_first_event"
                                                );
                                            }
                                            log_unknown_kiro_event(&event);
                                            let sse_events = ctx.process_kiro_event(&event);
                                            events.extend(sse_events);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("解码事件失败: {}", e);
                                    }
                                }
                            }

                            // 转换为 SSE 字节流
                            let bytes: Vec<Result<Bytes, Infallible>> = events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();

                            Some((stream::iter(bytes), (body_stream, ctx, decoder, false, first_event_logged, ping_interval, permit, stream_model, credential_id, stream_started_at)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            drop(permit);
                            // 发送最终事件并结束
                            let final_events = ctx.generate_final_events();
                            log_opus47_stream_diagnostics(ctx.opus47_diagnostics(), stream_started_at);
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, first_event_logged, ping_interval, None, stream_model, credential_id, stream_started_at)))
                        }
                        None => {
                            // 流结束，发送最终事件
                            drop(permit);
                            let final_events = ctx.generate_final_events_with_usage_commit();
                            log_opus47_stream_diagnostics(ctx.opus47_diagnostics(), stream_started_at);
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, first_event_logged, ping_interval, None, stream_model, credential_id, stream_started_at)))
                        }
                    }
                }
                // 发送 ping 保活
                _ = ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    Some((stream::iter(bytes), (body_stream, ctx, decoder, false, first_event_logged, ping_interval, permit, stream_model, credential_id, stream_started_at)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

fn log_opus47_stream_diagnostics(diagnostics: &Opus47Diagnostics, started_at: Instant) {
    if !diagnostics.enabled {
        return;
    }

    tracing::info!(
        target: "kiro_rs::metrics",
        model = %diagnostics.model,
        credential_id = diagnostics.credential_id,
        attempts = diagnostics.attempts,
        stabilization_mode = %diagnostics.stabilization_mode,
        client_thinking_enabled = diagnostics.client_thinking_enabled,
        assistant_response_count = diagnostics.assistant_response_count,
        reasoning_content_count = diagnostics.reasoning_content_count,
        tool_use_count = diagnostics.tool_use_count,
        signature_seen = diagnostics.signature_seen,
        visible_text_chars = diagnostics.visible_text_chars,
        hidden_reasoning_chars = diagnostics.hidden_reasoning_chars,
        first_event_type = diagnostics.first_event_type(),
        duration_ms = crate::metrics::duration_ms(started_at.elapsed()),
        "opus47_stream_diagnostics"
    );
}

fn log_opus47_nonstream_diagnostics(diagnostics: &Opus47Diagnostics, started_at: Instant) {
    if !diagnostics.enabled {
        return;
    }

    tracing::info!(
        target: "kiro_rs::metrics",
        model = %diagnostics.model,
        credential_id = diagnostics.credential_id,
        attempts = diagnostics.attempts,
        stabilization_mode = %diagnostics.stabilization_mode,
        client_thinking_enabled = diagnostics.client_thinking_enabled,
        assistant_response_count = diagnostics.assistant_response_count,
        reasoning_content_count = diagnostics.reasoning_content_count,
        tool_use_count = diagnostics.tool_use_count,
        signature_seen = diagnostics.signature_seen,
        visible_text_chars = diagnostics.visible_text_chars,
        hidden_reasoning_chars = diagnostics.hidden_reasoning_chars,
        first_event_type = diagnostics.first_event_type(),
        duration_ms = crate::metrics::duration_ms(started_at.elapsed()),
        "opus47_nonstream_diagnostics"
    );
}

fn event_metric_name(event: &Event) -> Cow<'static, str> {
    match event {
        Event::AssistantResponse(_) => Cow::Borrowed("assistant_response"),
        Event::ToolUse(_) => Cow::Borrowed("tool_use"),
        Event::Metering(_) => Cow::Borrowed("metering"),
        Event::ContextUsage(_) => Cow::Borrowed("context_usage"),
        Event::ReasoningContent(_) => Cow::Borrowed("reasoning_content"),
        Event::Unknown { event_type, .. } => Cow::Owned(event_type.clone()),
        Event::Error { .. } => Cow::Borrowed("error"),
        Event::Exception { .. } => Cow::Borrowed("exception"),
    }
}

fn log_unknown_kiro_event(event: &Event) {
    let Event::Unknown {
        event_type,
        payload,
    } = event
    else {
        return;
    };

    let payload_preview = String::from_utf8_lossy(payload);
    let payload_preview: String = payload_preview.chars().take(500).collect();
    tracing::warn!(
        event_type = %event_type,
        payload_len = payload.len(),
        payload_preview = %payload_preview,
        "收到未识别 Kiro 上游事件，已跳过"
    );
}

use super::converter::get_context_window_size;

fn build_virtual_usage(
    usage_manager: &VirtualCacheUsageManager,
    settings: &crate::kiro::settings::RuntimeSettings,
    credential_id: u64,
    model: &str,
    session_key: String,
    observed_total_input_tokens: i32,
    estimated_uncached_input_tokens: i32,
    output_tokens: i32,
    creation_ttl: CacheTtl,
) -> AnthropicUsage {
    usage_manager.build_usage(
        settings,
        VirtualUsageInput {
            credential_id,
            model: model.to_string(),
            session_key,
            observed_total_input_tokens,
            estimated_uncached_input_tokens: Some(estimated_uncached_input_tokens),
            output_tokens,
            creation_ttl,
        },
    )
}

/// 处理非流式请求
async fn handle_non_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    estimated_uncached_input_tokens: i32,
    extract_thinking: bool,
    client_thinking_enabled: bool,
    stabilization_mode: &str,
    opus47_diagnostics_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    session_id: Option<&str>,
    usage_session_key: String,
    usage_manager: Arc<VirtualCacheUsageManager>,
    request_ttl: CacheTtl,
    _permit: GlobalRequestPermit,
) -> Response {
    let request_started_at = Instant::now();
    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider
        .call_api_with_session(request_body, session_id, _permit.queue_ms())
        .await
    {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let credential_id = response.credential_id();
    let attempts = response.attempts();
    let mut opus47_diagnostics = Opus47Diagnostics::new(
        opus47_diagnostics_enabled,
        model,
        credential_id,
        attempts,
        stabilization_mode,
        client_thinking_enabled,
    );

    // 读取响应体
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取响应体失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("读取响应失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    // 解析事件流
    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    let mut text_content = String::new();
    let mut reasoning_content = String::new();
    let mut reasoning_signature: Option<String> = None;
    let mut tool_uses: Vec<serde_json::Value> = Vec::new();
    let mut has_tool_use = false;
    let mut stop_reason = "end_turn".to_string();
    // 从 contextUsageEvent 计算的实际输入 tokens
    let mut context_input_tokens: Option<i32> = None;

    // 收集工具调用的增量 JSON
    let mut tool_json_buffers: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => {
                if let Ok(event) = Event::from_frame(frame) {
                    opus47_diagnostics.observe_event(&event);
                    match event {
                        Event::AssistantResponse(resp) => {
                            text_content.push_str(&resp.content);
                        }
                        Event::ReasoningContent(reasoning) => {
                            if client_thinking_enabled {
                                if let Some(text) = reasoning.text {
                                    reasoning_content.push_str(&text);
                                }
                                if let Some(signature) = reasoning.signature {
                                    reasoning_signature = Some(signature);
                                }
                            }
                        }
                        Event::ToolUse(tool_use) => {
                            has_tool_use = true;

                            // 累积工具的 JSON 输入
                            let buffer = tool_json_buffers
                                .entry(tool_use.tool_use_id.clone())
                                .or_insert_with(String::new);
                            buffer.push_str(&tool_use.input);

                            // 如果是完整的工具调用，添加到列表
                            if tool_use.stop {
                                let input: serde_json::Value = if buffer.is_empty() {
                                    serde_json::json!({})
                                } else {
                                    serde_json::from_str(buffer).unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "工具输入 JSON 解析失败: {}, tool_use_id: {}",
                                            e,
                                            tool_use.tool_use_id
                                        );
                                        serde_json::json!({})
                                    })
                                };

                                let original_name = tool_name_map
                                    .get(&tool_use.name)
                                    .cloned()
                                    .unwrap_or_else(|| tool_use.name.clone());

                                tool_uses.push(json!({
                                    "type": "tool_use",
                                    "id": tool_use.tool_use_id,
                                    "name": original_name,
                                    "input": input
                                }));
                            }
                        }
                        Event::ContextUsage(context_usage) => {
                            // 从上下文使用百分比计算实际的 input_tokens
                            let window_size = get_context_window_size(model);
                            let actual_input_tokens =
                                (context_usage.context_usage_percentage * (window_size as f64)
                                    / 100.0) as i32;
                            context_input_tokens = Some(actual_input_tokens);
                            // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                            if context_usage.context_usage_percentage >= 100.0 {
                                stop_reason = "model_context_window_exceeded".to_string();
                            }
                            tracing::debug!(
                                "收到 contextUsageEvent: {}%, 计算 input_tokens: {}",
                                context_usage.context_usage_percentage,
                                actual_input_tokens
                            );
                        }
                        Event::Exception { exception_type, .. } => {
                            if exception_type == "ContentLengthExceededException" {
                                stop_reason = "max_tokens".to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                tracing::warn!("解码事件失败: {}", e);
            }
        }
    }

    // 确定 stop_reason
    if has_tool_use && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }

    // 构建响应内容
    let mut content: Vec<serde_json::Value> = Vec::new();

    if !reasoning_content.is_empty() || reasoning_signature.is_some() {
        let mut thinking = json!({
            "type": "thinking",
            "thinking": reasoning_content
        });
        if let Some(signature) = reasoning_signature {
            thinking["signature"] = json!(signature);
        }
        content.push(thinking);
    }

    if extract_thinking && reasoning_content.is_empty() {
        // 从完整文本中提取 thinking 块
        let (thinking, remaining_text) =
            super::stream::extract_thinking_from_complete_text(&text_content);

        if let Some(thinking_text) = thinking {
            content.push(json!({
                "type": "thinking",
                "thinking": thinking_text
            }));
        }

        if !remaining_text.is_empty() {
            content.push(json!({
                "type": "text",
                "text": remaining_text
            }));
        }
    } else if !text_content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

    content.extend(tool_uses);

    // 估算输出 tokens
    let output_tokens = token::estimate_output_tokens(&content);

    // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
    let final_input_tokens = context_input_tokens.unwrap_or(input_tokens);
    let settings = provider.token_manager().runtime_settings();
    let usage = build_virtual_usage(
        &usage_manager,
        &settings,
        credential_id,
        model,
        usage_session_key,
        final_input_tokens,
        estimated_uncached_input_tokens,
        output_tokens,
        request_ttl,
    );

    // 构建 Anthropic 响应
    let response_body = json!({
        "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage.to_json()
    });

    log_opus47_nonstream_diagnostics(&opus47_diagnostics, request_started_at);

    (StatusCode::OK, Json(response_body)).into_response()
}

/// 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
///
/// - Opus 4.6/4.7：覆写为 adaptive 类型
/// - 其他模型：覆写为 enabled 类型
/// - budget_tokens 固定为 20000
fn override_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model_lower = payload.model.to_lowercase();
    if !model_lower.contains("thinking") {
        return;
    }

    let is_opus_adaptive = model_lower.contains("opus")
        && (model_lower.contains("4-6")
            || model_lower.contains("4.6")
            || model_lower.contains("4-7")
            || model_lower.contains("4.7"));

    let thinking_type = if is_opus_adaptive {
        "adaptive"
    } else {
        "enabled"
    };

    tracing::info!(
        model = %payload.model,
        thinking_type = thinking_type,
        "模型名包含 thinking 后缀，覆写 thinking 配置"
    );

    payload.thinking = Some(Thinking {
        thinking_type: thinking_type.to_string(),
        budget_tokens: 20000,
    });

    if is_opus_adaptive {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
}

fn is_opus47_model_name(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "claude-opus-4-7"
            | "claude-opus-4.7"
            | "claude-opus-4-7-thinking"
            | "claude-opus-4.7-thinking"
    )
}

fn is_plain_opus47_model_name(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "claude-opus-4-7" | "claude-opus-4.7"
    )
}

fn client_thinking_enabled_for_request(model: &str, payload: &MessagesRequest) -> bool {
    if is_plain_opus47_model_name(model) {
        return false;
    }

    payload
        .thinking
        .as_ref()
        .map(|t| t.is_enabled())
        .unwrap_or(false)
}

fn apply_opus47_plain_stabilization(
    payload: &mut MessagesRequest,
    requested_model: &str,
    settings: &crate::kiro::settings::RuntimeSettings,
) -> String {
    let mode = crate::kiro::settings::normalize_opus47_plain_stabilization_mode(
        &settings.opus47_plain_stabilization_mode,
    );

    if !is_plain_opus47_model_name(requested_model) || mode == "off" {
        return "off".to_string();
    }

    let effort = match mode.as_str() {
        "adaptive_low" => "low",
        "adaptive_high" => "high",
        _ => return "off".to_string(),
    };

    payload.thinking = Some(Thinking {
        thinking_type: "adaptive".to_string(),
        budget_tokens: 20000,
    });
    payload.output_config = Some(OutputConfig {
        effort: effort.to_string(),
    });

    tracing::info!(
        model = %requested_model,
        stabilization_mode = %mode,
        effort = effort,
        "Opus 4.7 plain 稳定模式已注入 adaptive thinking，上游启用但客户端隐藏"
    );

    mode
}

/// POST /v1/messages/count_tokens
///
/// 计算消息的 token 数量
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    tracing::info!(
        model = %payload.model,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages/count_tokens request"
    );

    let total_tokens = token::count_all_tokens(
        payload.model,
        payload.system,
        payload.messages,
        payload.tools,
    ) as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1) as i32,
    })
}

/// POST /cc/v1/messages
///
/// Claude Code 兼容端点，与 /v1/messages 的区别在于：
/// - 流式响应会等待 kiro 端返回 contextUsageEvent 后再发送 message_start
/// - message_start 中的 input_tokens 是从 contextUsageEvent 计算的准确值
pub async fn post_messages_cc(
    State(state): State<AppState>,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /cc/v1/messages request"
    );

    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    let requested_model = payload.model.clone();
    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);
    let runtime_settings = provider.token_manager().runtime_settings();
    let usage_session_key = session_key_for_request(
        &payload,
        &payload.model,
        &runtime_settings.virtual_cache_fallback_scope,
    );
    let request_ttl =
        request_cache_ttl(&payload, CacheTtl::from_runtime_default(&runtime_settings));
    let estimated_uncached_input_tokens = estimate_latest_user_input_tokens(&payload);

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        let permit = match state
            .runtime_limiter
            .acquire(provider.token_manager())
            .await
        {
            Ok(permit) => permit,
            Err(e) => return e.into_response(),
        };

        return websearch::handle_websearch_request(provider, &payload, input_tokens, permit).await;
    }

    let stabilization_mode =
        apply_opus47_plain_stabilization(&mut payload, &requested_model, &runtime_settings);
    let client_thinking_enabled = client_thinking_enabled_for_request(&requested_model, &payload);
    let opus47_diagnostics_enabled =
        runtime_settings.opus47_diagnostics_enabled && is_opus47_model_name(&requested_model);

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求（profile_arn 由 provider 层根据实际凭据注入）
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: None,
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;

    let session_affinity_key = conversion_result.session_affinity_key;
    let tool_name_map = conversion_result.tool_name_map;

    let _permit = match state
        .runtime_limiter
        .acquire(provider.token_manager())
        .await
    {
        Ok(permit) => permit,
        Err(e) => return e.into_response(),
    };

    if payload.stream {
        // 流式响应（缓冲模式）
        handle_stream_request_buffered(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            estimated_uncached_input_tokens,
            client_thinking_enabled,
            stabilization_mode.as_str(),
            opus47_diagnostics_enabled,
            tool_name_map,
            Some(session_affinity_key.as_str()),
            usage_session_key,
            state.virtual_cache_usage.clone(),
            request_ttl,
            _permit,
        )
        .await
    } else {
        // 非流式响应：仅在配置开启时提取 thinking 块
        let extract_thinking = state.extract_thinking && client_thinking_enabled;
        handle_non_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            estimated_uncached_input_tokens,
            extract_thinking,
            client_thinking_enabled,
            stabilization_mode.as_str(),
            opus47_diagnostics_enabled,
            tool_name_map,
            Some(session_affinity_key.as_str()),
            usage_session_key,
            state.virtual_cache_usage.clone(),
            request_ttl,
            _permit,
        )
        .await
    }
}

/// 处理流式请求（缓冲版本）
///
/// 与 `handle_stream_request` 不同，此函数会缓冲所有事件直到流结束，
/// 然后用从 contextUsageEvent 计算的正确 input_tokens 生成 message_start 事件。
async fn handle_stream_request_buffered(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    estimated_input_tokens: i32,
    estimated_uncached_input_tokens: i32,
    client_thinking_enabled: bool,
    stabilization_mode: &str,
    opus47_diagnostics_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    session_id: Option<&str>,
    usage_session_key: String,
    usage_manager: Arc<VirtualCacheUsageManager>,
    request_ttl: CacheTtl,
    permit: GlobalRequestPermit,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider
        .call_api_stream_with_session(request_body, session_id, permit.queue_ms())
        .await
    {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let credential_id = response.credential_id();
    let attempts = response.attempts();
    let settings = provider.token_manager().runtime_settings();
    let model = model.to_string();

    // 创建缓冲流处理上下文
    let mut ctx = BufferedStreamContext::new(
        model.clone(),
        estimated_input_tokens,
        client_thinking_enabled,
        tool_name_map,
    );
    ctx.set_opus47_diagnostics(Opus47Diagnostics::new(
        opus47_diagnostics_enabled,
        model.clone(),
        credential_id,
        attempts,
        stabilization_mode,
        client_thinking_enabled,
    ));
    ctx.set_usage_builder(Box::new(
        move |final_input_tokens, output_tokens, commit_usage| {
            let pending_usage = usage_manager.preview_usage(
                &settings,
                VirtualUsageInput {
                    credential_id,
                    model: model.clone(),
                    session_key: usage_session_key,
                    observed_total_input_tokens: final_input_tokens,
                    estimated_uncached_input_tokens: Some(estimated_uncached_input_tokens),
                    output_tokens,
                    creation_ttl: request_ttl,
                },
            );
            let usage = pending_usage.usage().clone();
            if commit_usage {
                usage_manager.commit_usage(pending_usage);
            }
            usage
        },
    ));

    // 创建缓冲 SSE 流
    let stream = create_buffered_sse_stream(response, ctx, permit);

    // 返回 SSE 响应
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// 创建缓冲 SSE 事件流
///
/// 工作流程：
/// 1. 等待上游流完成，期间只发送 ping 保活信号
/// 2. 使用 StreamContext 的事件处理逻辑处理所有 Kiro 事件，结果缓存
/// 3. 流结束后，用正确的 input_tokens 更正 message_start 事件
/// 4. 一次性发送所有事件
fn create_buffered_sse_stream(
    response: crate::kiro::provider::LeasedResponse,
    ctx: BufferedStreamContext,
    permit: GlobalRequestPermit,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let credential_id = response.credential_id();
    let stream_model = ctx.model().to_string();
    let stream_started_at = Instant::now();
    let body_stream = response.bytes_stream();

    stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
            Some(permit),
            stream_model,
            credential_id,
            stream_started_at,
        ),
        |(
            mut body_stream,
            mut ctx,
            mut decoder,
            finished,
            mut first_event_logged,
            mut ping_interval,
            permit,
            stream_model,
            credential_id,
            stream_started_at,
        )| async move {
            if finished {
                return None;
            }

            loop {
                tokio::select! {
                    // 使用 biased 模式，优先检查 ping 定时器
                    // 避免在上游 chunk 密集时 ping 被"饿死"
                    biased;

                    // 优先检查 ping 保活（等待期间唯一发送的数据）
                    _ = ping_interval.tick() => {
                        tracing::trace!("发送 ping 保活事件（缓冲模式）");
                        let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                        return Some((stream::iter(bytes), (body_stream, ctx, decoder, false, first_event_logged, ping_interval, permit, stream_model, credential_id, stream_started_at)));
                    }

                    // 然后处理数据流
                    chunk_result = body_stream.next() => {
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                // 解码事件
                                if let Err(e) = decoder.feed(&chunk) {
                                    tracing::warn!("缓冲区溢出: {}", e);
                                }

                                for result in decoder.decode_iter() {
                                    match result {
                                        Ok(frame) => {
                                            if let Ok(event) = Event::from_frame(frame) {
                                                if !first_event_logged {
                                                    first_event_logged = true;
                                                    tracing::info!(
                                                        target: "kiro_rs::metrics",
                                                        model = %stream_model,
                                                        stream = true,
                                                        credential_id,
                                                        buffered = true,
                                                        first_event_ms = crate::metrics::duration_ms(stream_started_at.elapsed()),
                                                        event_type = %event_metric_name(&event),
                                                        "upstream_stream_first_event"
                                                    );
                                                }
                                                log_unknown_kiro_event(&event);
                                                // 缓冲事件（复用 StreamContext 的处理逻辑）
                                                ctx.process_and_buffer(&event);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("解码事件失败: {}", e);
                                        }
                                    }
                                }
                                // 继续读取下一个 chunk，不发送任何数据
                            }
                            Some(Err(e)) => {
                                tracing::error!("读取响应流失败: {}", e);
                                drop(permit);
                                // 发生错误，完成处理并返回所有事件
                                let all_events = ctx.finish_and_get_all_events();
                                log_opus47_stream_diagnostics(ctx.opus47_diagnostics(), stream_started_at);
                                let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                    .into_iter()
                                    .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, first_event_logged, ping_interval, None, stream_model, credential_id, stream_started_at)));
                            }
                            None => {
                                // 流结束，完成处理并返回所有事件（已更正 input_tokens）
                                drop(permit);
                                let all_events = ctx.finish_and_get_all_events_with_usage_commit();
                                log_opus47_stream_diagnostics(ctx.opus47_diagnostics(), stream_started_at);
                                let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                    .into_iter()
                                    .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, first_event_logged, ping_interval, None, stream_model, credential_id, stream_started_at)));
                            }
                        }
                    }
                }
            }
        },
    )
    .flatten()
}
