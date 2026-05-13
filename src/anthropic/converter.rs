//! Anthropic → Kiro 协议转换器
//!
//! 负责将 Anthropic API 请求格式转换为 Kiro API 请求格式

use std::collections::HashMap;
use std::io::Read;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::{DeflateDecoder, ZlibDecoder};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::kiro::model::requests::conversation::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, KiroImage, Message, UserInputMessage, UserInputMessageContext, UserMessage,
};
use crate::kiro::model::requests::tool::{
    InputSchema, Tool, ToolResult, ToolSpecification, ToolUseEntry,
};

use super::types::{ContentBlock, MessagesRequest, StructuredOutputFormat};

/// 规范化 JSON Schema，修复 MCP 工具定义中常见的类型问题
///
/// Claude Code / MCP 工具定义偶尔会出现 `required: null`、`properties: null` 等，
/// 导致上游返回 400 "Improperly formed request"。
fn normalize_json_schema(schema: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut obj) = schema else {
        return serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": true
        });
    };

    // type（必须是字符串）
    if !obj
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("object".to_string()),
        );
    }

    // properties（必须是 object）
    match obj.get("properties") {
        Some(serde_json::Value::Object(_)) => {}
        _ => {
            obj.insert(
                "properties".to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }
    }

    // required（必须是 string 数组）
    let required = match obj.remove("required") {
        Some(serde_json::Value::Array(arr)) => serde_json::Value::Array(
            arr.into_iter()
                .filter_map(|v| v.as_str().map(|s| serde_json::Value::String(s.to_string())))
                .collect(),
        ),
        _ => serde_json::Value::Array(Vec::new()),
    };
    obj.insert("required".to_string(), required);

    // additionalProperties（允许 bool 或 object，其他按 true 处理）
    match obj.get("additionalProperties") {
        Some(serde_json::Value::Bool(_)) | Some(serde_json::Value::Object(_)) => {}
        _ => {
            obj.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }

    serde_json::Value::Object(obj)
}

/// 追加到 Write 工具 description 末尾的内容
const WRITE_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the content to write exceeds 150 lines, you MUST only write the first 50 lines using this tool, then use `Edit` tool to append the remaining content in chunks of no more than 50 lines each. If needed, leave a unique placeholder to help append content. Do NOT attempt to write all content at once.";

/// 追加到 Edit 工具 description 末尾的内容
const EDIT_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the `new_string` content exceeds 50 lines, you MUST split it into multiple Edit calls, each replacing no more than 50 lines at a time. If used to append content, leave a unique placeholder to help append content. On the final chunk, do NOT include the placeholder.";

/// 追加到系统提示词的分块写入策略
const SYSTEM_CHUNKED_POLICY: &str = "\
When the Write or Edit tool has content size limits, always comply silently. \
Never suggest bypassing these limits via alternative tools. \
Never ask the user whether to switch approaches. \
Complete all chunked operations without commentary.";

/// 单个 PDF base64 文档的最大解码字节数。
const MAX_PDF_BYTES: usize = 25 * 1024 * 1024;

/// 注入给模型的 PDF 文本最大字符数，避免测试外的大文档把上下文打爆。
const MAX_PDF_EXTRACTED_CHARS: usize = 120_000;

/// pdf_extract 有时会在坏 xref 或简化 PDF 上只吐出页码/少量残片。
const MIN_PDF_PRIMARY_TEXT_CHARS: usize = 32;

/// 模型映射：将 Anthropic 模型名映射到 Kiro 模型 ID
///
/// 按照用户要求：
/// - sonnet 4.6/4-6 → claude-sonnet-4.6
/// - 其他 sonnet → claude-sonnet-4.5
/// - opus 4.7/4-7 → claude-opus-4.7
/// - opus 4.5/4-5 → claude-opus-4.5
/// - 其他 opus → claude-opus-4.6
/// - 所有 haiku → claude-haiku-4.5
pub fn map_model(model: &str) -> Option<String> {
    let model_lower = model.to_lowercase();

    if model_lower.contains("sonnet") {
        if model_lower.contains("4-6") || model_lower.contains("4.6") {
            Some("claude-sonnet-4.6".to_string())
        } else {
            Some("claude-sonnet-4.5".to_string())
        }
    } else if model_lower.contains("opus") {
        if model_lower.contains("4-7") || model_lower.contains("4.7") {
            Some("claude-opus-4.7".to_string())
        } else if model_lower.contains("4-5") || model_lower.contains("4.5") {
            Some("claude-opus-4.5".to_string())
        } else {
            Some("claude-opus-4.6".to_string())
        }
    } else if model_lower.contains("haiku") {
        Some("claude-haiku-4.5".to_string())
    } else {
        None
    }
}

/// 根据模型名称返回对应的上下文窗口大小
///
/// 复用 `map_model` 的映射逻辑，确保窗口大小判断与模型映射一致。
/// Kiro 于 2026-03-24 将 Opus 4.6 和 Sonnet 4.6 升级至 1M 上下文。
/// Opus 4.7 沿用 1M 上下文窗口。
pub fn get_context_window_size(model: &str) -> i32 {
    match map_model(model) {
        Some(mapped)
            if mapped == "claude-sonnet-4.6"
                || mapped == "claude-opus-4.6"
                || mapped == "claude-opus-4.7" =>
        {
            1_000_000
        }
        _ => 200_000,
    }
}

/// 转换结果
#[derive(Debug)]
pub struct ConversionResult {
    /// 转换后的 Kiro 请求
    pub conversation_state: ConversationState,
    /// 用于账号软亲和的会话 ID
    pub session_affinity_key: String,
    /// 工具名称映射（短名称 → 原始名称），仅当存在超长工具名时非空
    pub tool_name_map: HashMap<String, String>,
    /// PDF 调试摘要。仅当当前请求包含 PDF 文档时存在。
    pub pdf_debug: Option<PdfDebugInfo>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConversionOptions {
    pub clean_probe_mode: bool,
}

#[derive(Debug, Clone)]
pub struct PdfDebugInfo {
    pub name: String,
    pub page_count: Option<usize>,
    pub text_source: &'static str,
    pub extracted_chars: usize,
    pub extracted_text: String,
    pub text_preview: String,
}

/// 转换错误
#[derive(Debug)]
pub enum ConversionError {
    UnsupportedModel(String),
    EmptyMessages,
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversionError::UnsupportedModel(model) => write!(f, "模型不支持: {}", model),
            ConversionError::EmptyMessages => write!(f, "消息列表为空"),
        }
    }
}

impl std::error::Error for ConversionError {}

/// 从 metadata.user_id 中提取 session UUID
///
/// 支持两种格式:
/// 1. 字符串格式: user_xxx_account__session_0b4445e1-f5be-49e1-87ce-62bbc28ad705
/// 2. JSON 格式: {"device_id":"...","account_uuid":"...","session_id":"UUID"}
///
/// 提取 session UUID 作为 conversationId
pub fn extract_session_id(user_id: &str) -> Option<String> {
    // 先尝试 JSON 解析
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(user_id) {
        if let Some(session_id) = json.get("session_id").and_then(|v| v.as_str()) {
            if is_valid_uuid(session_id) {
                return Some(session_id.to_string());
            }
        }
    }

    // 回退到字符串格式: 查找 "session_" 后面的内容
    if let Some(pos) = user_id.find("session_") {
        let session_part = &user_id[pos + 8..]; // "session_" 长度为 8
        if session_part.len() >= 36 {
            let uuid_str = &session_part[..36];
            if is_valid_uuid(uuid_str) {
                return Some(uuid_str.to_string());
            }
        }
    }
    None
}

/// 简单验证 UUID 格式（36 字符，包含 4 个连字符）
fn is_valid_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4
}

/// 收集历史消息中使用的所有工具名称
fn collect_history_tool_names(history: &[Message]) -> Vec<String> {
    let mut tool_names = Vec::new();

    for msg in history {
        if let Message::Assistant(assistant_msg) = msg {
            if let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                for tool_use in tool_uses {
                    if !tool_names.contains(&tool_use.name) {
                        tool_names.push(tool_use.name.clone());
                    }
                }
            }
        }
    }

    tool_names
}

/// 为历史中使用但不在 tools 列表中的工具创建占位符定义
/// Kiro API 要求：历史消息中引用的工具必须在 currentMessage.tools 中有定义
fn create_placeholder_tool(name: &str) -> Tool {
    Tool {
        tool_specification: ToolSpecification {
            name: name.to_string(),
            description: "Tool used in conversation history".to_string(),
            input_schema: InputSchema::from_json(serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": true
            })),
        },
    }
}

/// 将 Anthropic 请求转换为 Kiro 请求
pub fn convert_request(req: &MessagesRequest) -> Result<ConversionResult, ConversionError> {
    convert_request_with_options(req, ConversionOptions::default())
}

/// 将 Anthropic 请求转换为 Kiro 请求，并应用调用方提供的兼容选项。
pub fn convert_request_with_options(
    req: &MessagesRequest,
    options: ConversionOptions,
) -> Result<ConversionResult, ConversionError> {
    // 1. 映射模型
    let model_id = map_model(&req.model)
        .ok_or_else(|| ConversionError::UnsupportedModel(req.model.clone()))?;

    // 2. 检查消息列表
    if req.messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }

    // 2.5. 预处理 prefill：如果末尾是 assistant，静默丢弃并截断到最后一条 user
    // Claude 4.x 已弃用 assistant prefill，Kiro API 也不支持
    let messages: &[_] = if req.messages.last().is_some_and(|m| m.role != "user") {
        tracing::info!("检测到末尾 assistant 消息（prefill），静默丢弃");
        let last_user_idx = req
            .messages
            .iter()
            .rposition(|m| m.role == "user")
            .ok_or(ConversionError::EmptyMessages)?;
        &req.messages[..=last_user_idx]
    } else {
        &req.messages
    };

    // 3. 生成会话 ID 和代理 ID
    // 优先从 metadata.user_id 中提取 session UUID 作为 conversationId
    let conversation_id = req
        .metadata
        .as_ref()
        .and_then(|m| m.user_id.as_ref())
        .and_then(|user_id| extract_session_id(user_id))
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    // Kiro 官方客户端保持 agentContinuationId 与 conversationId 稳定一致。
    // 随机 continuation id 会削弱同一会话在上游的连续性与缓存局部性。
    let agent_continuation_id = conversation_id.clone();

    // 4. 确定触发类型
    let chat_trigger_type = determine_chat_trigger_type(req);

    // 5. 处理最后一条消息作为 current_message（经过 prefill 预处理，末尾必为 user）
    let last_message = messages.last().unwrap();
    let processed_content = process_message_content(&last_message.content)?;
    let text_content = apply_current_message_prefixes(req, processed_content.text, options);
    let images = processed_content.images;
    let tool_results = processed_content.tool_results;
    let pdf_debug = processed_content.pdf_debug;
    if let Some(pdf_debug) = pdf_debug.as_ref() {
        tracing::warn!(
            name = %pdf_debug.name,
            page_count = pdf_debug.page_count.unwrap_or(0),
            text_source = pdf_debug.text_source,
            extracted_chars = pdf_debug.extracted_chars,
            pdf_text_preview = %pdf_debug.text_preview,
            "PDF 请求诊断"
        );
    }

    // 6. 转换工具定义（超长名称自动缩短并记录映射）
    let mut tool_name_map = HashMap::new();
    let mut tools = convert_tools(&req.tools, &mut tool_name_map, options);

    // 7. 构建历史消息（需要先构建，以便收集历史中使用的工具）
    let mut history = build_history(req, messages, &model_id, &mut tool_name_map, options)?;

    // 8. 验证并过滤 tool_use/tool_result 配对
    // 移除孤立的 tool_result（没有对应的 tool_use）
    // 同时返回孤立的 tool_use_id 集合，用于后续清理
    let (validated_tool_results, orphaned_tool_use_ids) =
        validate_tool_pairing(&history, &tool_results);

    // 9. 从历史中移除孤立的 tool_use（Kiro API 要求 tool_use 必须有对应的 tool_result）
    remove_orphaned_tool_uses(&mut history, &orphaned_tool_use_ids);

    // 10. 收集历史中使用的工具名称，为缺失的工具生成占位符定义
    // Kiro API 要求：历史消息中引用的工具必须在 tools 列表中有定义
    // 注意：Kiro 匹配工具名称时忽略大小写，所以这里也需要忽略大小写比较
    let history_tool_names = collect_history_tool_names(&history);
    let existing_tool_names: std::collections::HashSet<_> = tools
        .iter()
        .map(|t| t.tool_specification.name.to_lowercase())
        .collect();

    for tool_name in history_tool_names {
        if !existing_tool_names.contains(&tool_name.to_lowercase()) {
            tools.push(create_placeholder_tool(&tool_name));
        }
    }

    // 11. 构建 UserInputMessageContext
    let mut context = UserInputMessageContext::new();
    if !tools.is_empty() {
        context = context.with_tools(tools);
    }
    if !validated_tool_results.is_empty() {
        context = context.with_tool_results(validated_tool_results);
    }

    // 12. 构建当前消息
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let content = text_content;

    let mut user_input = UserInputMessage::new(content, &model_id)
        .with_context(context)
        .with_origin("AI_EDITOR");

    if !images.is_empty() {
        user_input = user_input.with_images(images);
    }

    let current_message = CurrentMessage::new(user_input);

    // 13. 构建 ConversationState
    let session_affinity_key = conversation_id.clone();
    let conversation_state = ConversationState::new(conversation_id)
        .with_agent_continuation_id(agent_continuation_id)
        .with_agent_task_type("vibe")
        .with_chat_trigger_type(chat_trigger_type)
        .with_current_message(current_message)
        .with_history(history);

    if !tool_name_map.is_empty() {
        tracing::info!("工具名称映射: {} 个超长名称已缩短", tool_name_map.len());
    }

    Ok(ConversionResult {
        conversation_state,
        session_affinity_key,
        tool_name_map,
        pdf_debug,
    })
}

struct ProcessedMessageContent {
    text: String,
    images: Vec<KiroImage>,
    tool_results: Vec<ToolResult>,
    pdf_debug: Option<PdfDebugInfo>,
}

/// 确定聊天触发类型
/// "AUTO" 模式可能会导致 400 Bad Request 错误
fn determine_chat_trigger_type(_req: &MessagesRequest) -> String {
    "MANUAL".to_string()
}

/// 处理消息内容，提取文本、图片、文档和工具结果
fn process_message_content(
    content: &serde_json::Value,
) -> Result<ProcessedMessageContent, ConversionError> {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    let mut pdf_debug = None;

    match content {
        serde_json::Value::String(s) => {
            text_parts.push(s.clone());
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "text" => {
                            if let Some(text) = block.text {
                                text_parts.push(text);
                            }
                        }
                        "image" => {
                            if let Some(source) = block.source {
                                if is_pdf_media_type(&source.media_type) {
                                    if let Some(pdf) = process_pdf_document_source(
                                        &source.data,
                                        block
                                            .title
                                            .as_deref()
                                            .or(block.name.as_deref())
                                            .unwrap_or("document.pdf"),
                                    ) {
                                        pdf_debug = Some(pdf.debug);
                                        text_parts.push(pdf.text);
                                    }
                                } else if let Some(format) = get_image_format(&source.media_type) {
                                    images.push(KiroImage::from_base64(format, source.data));
                                }
                            }
                        }
                        "document" => {
                            if let Some(source) = block.source {
                                if is_pdf_media_type(&source.media_type) {
                                    let name = block
                                        .title
                                        .as_deref()
                                        .or(block.name.as_deref())
                                        .unwrap_or("document.pdf");
                                    if let Some(pdf) =
                                        process_pdf_document_source(&source.data, name)
                                    {
                                        pdf_debug = Some(pdf.debug);
                                        text_parts.push(pdf.text);
                                    }
                                } else {
                                    text_parts.push(format!(
                                        "[Document \"{}\" — unsupported media type: {}]",
                                        block
                                            .title
                                            .as_deref()
                                            .or(block.name.as_deref())
                                            .unwrap_or("document"),
                                        source.media_type
                                    ));
                                }
                            }
                        }
                        "tool_result" => {
                            if let Some(tool_use_id) = block.tool_use_id {
                                let result_content = extract_tool_result_content(&block.content);
                                let is_error = block.is_error.unwrap_or(false);

                                let mut result = if is_error {
                                    ToolResult::error(&tool_use_id, result_content)
                                } else {
                                    ToolResult::success(&tool_use_id, result_content)
                                };
                                result.status =
                                    Some(if is_error { "error" } else { "success" }.to_string());

                                tool_results.push(result);
                            }
                        }
                        "tool_use" => {
                            // tool_use 在 assistant 消息中处理，这里忽略
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    Ok(ProcessedMessageContent {
        text: text_parts.join("\n"),
        images,
        tool_results,
        pdf_debug,
    })
}

struct ProcessedPdfDocument {
    text: String,
    debug: PdfDebugInfo,
}

fn is_pdf_media_type(media_type: &str) -> bool {
    media_type.eq_ignore_ascii_case("application/pdf")
}

fn process_pdf_document_source(data: &str, name: &str) -> Option<ProcessedPdfDocument> {
    let decoded_len = estimate_base64_decoded_len(data);
    if decoded_len > MAX_PDF_BYTES {
        tracing::warn!(
            name = name,
            decoded_len,
            max_bytes = MAX_PDF_BYTES,
            "PDF 文档超过大小限制，跳过文本提取"
        );
        return Some(ProcessedPdfDocument {
            text: format!(
                "[PDF Document \"{}\" — skipped because decoded size exceeds {} bytes]",
                name, MAX_PDF_BYTES
            ),
            debug: PdfDebugInfo {
                name: name.to_string(),
                page_count: None,
                text_source: "skipped",
                extracted_chars: 0,
                extracted_text: String::new(),
                text_preview: String::new(),
            },
        });
    }

    let bytes = match STANDARD.decode(data) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(name = name, error = %err, "PDF base64 解码失败");
            return Some(ProcessedPdfDocument {
                text: format!("[PDF Document \"{}\" — invalid base64 data]", name),
                debug: PdfDebugInfo {
                    name: name.to_string(),
                    page_count: None,
                    text_source: "invalid_base64",
                    extracted_chars: 0,
                    extracted_text: String::new(),
                    text_preview: String::new(),
                },
            });
        }
    };

    if !bytes.starts_with(b"%PDF-") {
        tracing::warn!(
            name = name,
            "document media_type 是 application/pdf 但内容不是 PDF"
        );
        return Some(ProcessedPdfDocument {
            text: format!("[PDF Document \"{}\" — invalid PDF data]", name),
            debug: PdfDebugInfo {
                name: name.to_string(),
                page_count: None,
                text_source: "invalid_pdf",
                extracted_chars: 0,
                extracted_text: String::new(),
                text_preview: String::new(),
            },
        });
    }

    let page_count = count_pdf_pages(&bytes);
    let mut primary_error = None;
    let extracted = match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => text,
        Err(err) => {
            primary_error = Some(err.to_string());
            String::new()
        }
    };

    let fallback = extract_simple_pdf_text_ops(&bytes);
    let fallback_text = fallback.text.as_str();
    let selection = select_pdf_text(&extracted, &fallback_text);
    let text = selection.text.trim();
    if let Some(err) = primary_error.as_deref() {
        if text.is_empty() {
            tracing::warn!(
                name = name,
                error = %err,
                fallback_chars = selection.fallback_chars,
                "PDF 主解析器失败，fallback 也未提取到文本"
            );
        } else {
            tracing::info!(
                name = name,
                error = %err,
                text_source = selection.source,
                fallback_chars = selection.fallback_chars,
                extracted_chars = text.chars().count(),
                "PDF 主解析器失败，已使用 fallback 文本"
            );
        }
    }
    let content = if text.is_empty() {
        format!(
            "[PDF Document \"{}\" — no extractable text{}]",
            name,
            page_count
                .map(|count| format!(", {} page(s)", count))
                .unwrap_or_default()
        )
    } else {
        let truncated = truncate_chars(text, MAX_PDF_EXTRACTED_CHARS);
        let truncation_note = if truncated.len() < text.len() {
            format!(
                "\n[PDF text truncated to {} characters]",
                MAX_PDF_EXTRACTED_CHARS
            )
        } else {
            String::new()
        };
        let page_count_line = page_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "[PDF Document \"{}\"]\nPage count: {}\nExtracted text:\n{}\n[End PDF Document \"{}\"]{}",
            name, page_count_line, truncated, name, truncation_note
        )
    };

    tracing::info!(
        name = name,
        page_count = page_count.unwrap_or(0),
        text_source = selection.source,
        primary_chars = selection.primary_chars,
        fallback_chars = selection.fallback_chars,
        fallback_streams = fallback.stream_count,
        fallback_decoded_streams = fallback.decoded_stream_count,
        extracted_chars = text.chars().count(),
        "PDF 文档文本已提取并注入到当前消息"
    );
    tracing::debug!(
        name = name,
        extracted_preview = %pdf_text_preview(text),
        "PDF 文档文本提取预览"
    );
    if text.chars().count() < MIN_PDF_PRIMARY_TEXT_CHARS {
        log_pdf_low_text_diagnostics(name, &bytes, &fallback);
    }

    let debug = PdfDebugInfo {
        name: name.to_string(),
        page_count,
        text_source: selection.source,
        extracted_chars: text.chars().count(),
        extracted_text: text.to_string(),
        text_preview: pdf_text_preview(text),
    };

    Some(ProcessedPdfDocument {
        text: content,
        debug,
    })
}

struct PdfTextSelection {
    text: String,
    source: &'static str,
    primary_chars: usize,
    fallback_chars: usize,
}

fn select_pdf_text(primary: &str, fallback: &str) -> PdfTextSelection {
    let primary = primary.trim();
    let fallback = fallback.trim();
    let primary_chars = primary.chars().count();
    let fallback_chars = fallback.chars().count();

    if primary_chars == 0 {
        return PdfTextSelection {
            text: fallback.to_string(),
            source: "fallback",
            primary_chars,
            fallback_chars,
        };
    }

    if fallback_chars == 0 {
        return PdfTextSelection {
            text: primary.to_string(),
            source: "primary",
            primary_chars,
            fallback_chars,
        };
    }

    let fallback_is_materially_better = (primary_chars < MIN_PDF_PRIMARY_TEXT_CHARS
        && fallback_chars > primary_chars)
        || fallback_chars
            >= primary_chars
                .saturating_mul(2)
                .max(MIN_PDF_PRIMARY_TEXT_CHARS);

    if fallback_is_materially_better {
        if primary_chars < MIN_PDF_PRIMARY_TEXT_CHARS
            || normalized_pdf_text_contains(fallback, primary)
        {
            return PdfTextSelection {
                text: fallback.to_string(),
                source: "fallback",
                primary_chars,
                fallback_chars,
            };
        }

        return PdfTextSelection {
            text: format!("{}\n{}", primary, fallback),
            source: "combined",
            primary_chars,
            fallback_chars,
        };
    }

    PdfTextSelection {
        text: primary.to_string(),
        source: "primary",
        primary_chars,
        fallback_chars,
    }
}

fn normalized_pdf_text_contains(haystack: &str, needle: &str) -> bool {
    let haystack = normalize_pdf_text_for_compare(haystack);
    let needle = normalize_pdf_text_for_compare(needle);
    needle.is_empty() || haystack.contains(&needle)
}

fn normalize_pdf_text_for_compare(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn pdf_text_preview(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&normalized, 240)
}

fn estimate_base64_decoded_len(data: &str) -> usize {
    let trimmed = data.trim();
    let padding = trimmed
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    trimmed
        .len()
        .saturating_div(4)
        .saturating_mul(3)
        .saturating_sub(padding)
}

fn count_pdf_pages(bytes: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(bytes);
    let count = count_pdf_type_page_markers(&text);
    (count > 0).then_some(count)
}

fn count_pdf_type_page_markers(text: &str) -> usize {
    let mut count = 0usize;
    let mut rest = text;

    while let Some(idx) = rest.find("/Type") {
        rest = &rest[idx + "/Type".len()..];
        let after_type = rest.trim_start();
        if let Some(after_page) = after_type.strip_prefix("/Page") {
            if after_page.chars().next().is_none_or(is_pdf_name_delimiter) {
                count += 1;
            }
        }
    }

    count
}

fn is_pdf_name_delimiter(ch: char) -> bool {
    ch.is_ascii_whitespace() || matches!(ch, '/' | '<' | '>' | '[' | ']' | '(' | ')' | '%')
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => text[..idx].to_string(),
        None => text.to_string(),
    }
}

struct PdfFallbackText {
    text: String,
    stream_count: usize,
    decoded_stream_count: usize,
    stream_diagnostics: Vec<PdfStreamDiagnostic>,
}

fn extract_simple_pdf_text_ops(bytes: &[u8]) -> PdfFallbackText {
    let mut out = Vec::new();
    extract_text_ops_from_latin1(&String::from_utf8_lossy(bytes), &mut out);

    let mut stream_count = 0usize;
    let mut decoded_stream_count = 0usize;
    for stream in extract_pdf_streams(bytes) {
        stream_count += 1;
        for decoded in decode_pdf_stream_candidates(&stream) {
            if decoded.data.as_slice() != stream.data {
                decoded_stream_count += 1;
            }
            extract_text_ops_from_latin1(&String::from_utf8_lossy(&decoded.data), &mut out);
        }
    }

    PdfFallbackText {
        text: dedupe_preserve_order(out).join("\n"),
        stream_count,
        decoded_stream_count,
        stream_diagnostics: collect_pdf_stream_diagnostics(bytes),
    }
}

struct PdfStreamDiagnostic {
    index: usize,
    dict_preview: String,
    data_len: usize,
    filter_summary: String,
    subtype_summary: String,
    has_bt: bool,
    has_tj: bool,
    has_tj_array: bool,
    has_do: bool,
    ascii_preview: String,
    hex_prefix: String,
}

struct PdfStream<'a> {
    is_flate: bool,
    is_ascii_hex: bool,
    is_ascii85: bool,
    data: &'a [u8],
}

fn extract_pdf_streams(bytes: &[u8]) -> Vec<PdfStream<'_>> {
    let mut streams = Vec::new();
    let mut pos = 0usize;

    while let Some(relative_start) = find_subslice(&bytes[pos..], b"stream") {
        let stream_keyword = pos + relative_start;
        let after_keyword = stream_keyword + b"stream".len();
        let data_start = if bytes.get(after_keyword) == Some(&b'\r')
            && bytes.get(after_keyword + 1) == Some(&b'\n')
        {
            after_keyword + 2
        } else if bytes.get(after_keyword) == Some(&b'\n')
            || bytes.get(after_keyword) == Some(&b'\r')
        {
            after_keyword + 1
        } else {
            after_keyword
        };

        let Some(relative_end) = find_subslice(&bytes[data_start..], b"endstream") else {
            break;
        };
        let mut data_end = data_start + relative_end;
        while data_end > data_start && matches!(bytes[data_end - 1], b'\n' | b'\r') {
            data_end -= 1;
        }

        let dict_start = stream_keyword.saturating_sub(1024);
        let dict = String::from_utf8_lossy(&bytes[dict_start..stream_keyword]);
        streams.push(PdfStream {
            is_flate: dict.contains("FlateDecode"),
            is_ascii_hex: dict.contains("ASCIIHexDecode"),
            is_ascii85: dict.contains("ASCII85Decode"),
            data: &bytes[data_start..data_end],
        });

        pos = data_start + relative_end + b"endstream".len();
    }

    streams
}

fn collect_pdf_stream_diagnostics(bytes: &[u8]) -> Vec<PdfStreamDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut pos = 0usize;

    while let Some(relative_start) = find_subslice(&bytes[pos..], b"stream") {
        let stream_keyword = pos + relative_start;
        let after_keyword = stream_keyword + b"stream".len();
        let data_start = if bytes.get(after_keyword) == Some(&b'\r')
            && bytes.get(after_keyword + 1) == Some(&b'\n')
        {
            after_keyword + 2
        } else if bytes.get(after_keyword) == Some(&b'\n')
            || bytes.get(after_keyword) == Some(&b'\r')
        {
            after_keyword + 1
        } else {
            after_keyword
        };

        let Some(relative_end) = find_subslice(&bytes[data_start..], b"endstream") else {
            break;
        };
        let mut data_end = data_start + relative_end;
        while data_end > data_start && matches!(bytes[data_end - 1], b'\n' | b'\r') {
            data_end -= 1;
        }

        let dict_start = stream_keyword.saturating_sub(1024);
        let dict = String::from_utf8_lossy(&bytes[dict_start..stream_keyword]);
        let data = &bytes[data_start..data_end];
        diagnostics.push(PdfStreamDiagnostic {
            index: diagnostics.len(),
            dict_preview: pdf_ascii_preview(dict.as_bytes(), 400),
            data_len: data.len(),
            filter_summary: extract_pdf_name_after_key(&dict, "/Filter")
                .unwrap_or_else(|| "none".to_string()),
            subtype_summary: extract_pdf_name_after_key(&dict, "/Subtype")
                .unwrap_or_else(|| "none".to_string()),
            has_bt: find_subslice(data, b"BT").is_some(),
            has_tj: find_subslice(data, b"Tj").is_some(),
            has_tj_array: find_subslice(data, b"TJ").is_some(),
            has_do: find_subslice(data, b" Do").is_some() || data.ends_with(b"Do"),
            ascii_preview: pdf_ascii_preview(data, 240),
            hex_prefix: pdf_hex_prefix(data, 64),
        });

        pos = data_start + relative_end + b"endstream".len();
    }

    diagnostics
}

fn extract_pdf_name_after_key(dict: &str, key: &str) -> Option<String> {
    let start = dict.rfind(key)?;
    let rest = dict[start + key.len()..].trim_start();
    if let Some(array) = rest.strip_prefix('[') {
        let end = array.find(']').unwrap_or(array.len());
        return Some(
            array[..end]
                .split_whitespace()
                .take(8)
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    rest.split_whitespace()
        .next()
        .map(|value| value.trim_matches(|ch| ch == '<' || ch == '>').to_string())
}

fn log_pdf_low_text_diagnostics(name: &str, bytes: &[u8], fallback: &PdfFallbackText) {
    let pdf_text = String::from_utf8_lossy(bytes);
    tracing::warn!(
        name = name,
        pdf_bytes = bytes.len(),
        page_markers = count_pdf_pages(bytes).unwrap_or(0),
        obj_count = pdf_text.matches(" obj").count(),
        endobj_count = pdf_text.matches("endobj").count(),
        stream_count = fallback.stream_count,
        decoded_stream_count = fallback.decoded_stream_count,
        fallback_chars = fallback.text.chars().count(),
        has_xobject = pdf_text.contains("/XObject"),
        has_image_subtype =
            pdf_text.contains("/Subtype /Image") || pdf_text.contains("/Subtype/Image"),
        has_tounicode = pdf_text.contains("/ToUnicode"),
        has_flate = pdf_text.contains("FlateDecode"),
        has_dct = pdf_text.contains("DCTDecode"),
        has_jpx = pdf_text.contains("JPXDecode"),
        has_objstm = pdf_text.contains("/ObjStm"),
        "PDF 低文本提取诊断"
    );

    for stream in fallback.stream_diagnostics.iter().take(8) {
        tracing::warn!(
            name = name,
            stream_index = stream.index,
            data_len = stream.data_len,
            filter = %stream.filter_summary,
            subtype = %stream.subtype_summary,
            has_bt = stream.has_bt,
            has_tj = stream.has_tj,
            has_tj_array = stream.has_tj_array,
            has_do = stream.has_do,
            dict = %stream.dict_preview,
            data_ascii_preview = %stream.ascii_preview,
            data_hex_prefix = %stream.hex_prefix,
            "PDF stream 诊断"
        );
    }
}

fn pdf_ascii_preview(bytes: &[u8], max_chars: usize) -> String {
    let mut preview = String::new();
    for byte in bytes.iter().copied() {
        let ch = match byte {
            b'\n' | b'\r' | b'\t' => ' ',
            0x20..=0x7e => byte as char,
            _ => '.',
        };
        preview.push(ch);
        if preview.chars().count() >= max_chars {
            break;
        }
    }
    preview.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pdf_hex_prefix(bytes: &[u8], max_bytes: usize) -> String {
    bytes
        .iter()
        .take(max_bytes)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

struct PdfDecodedStream {
    data: Vec<u8>,
}

fn decode_pdf_stream_candidates(stream: &PdfStream<'_>) -> Vec<PdfDecodedStream> {
    let mut candidates = Vec::new();
    push_pdf_stream_candidate(&mut candidates, stream.data.to_vec());

    let mut seeds = vec![stream.data.to_vec()];
    if stream.is_ascii_hex {
        if let Some(decoded) = decode_ascii_hex_bytes(stream.data) {
            push_pdf_stream_candidate(&mut candidates, decoded.clone());
            seeds.push(decoded);
        }
    }
    if stream.is_ascii85 {
        if let Some(decoded) = decode_ascii85_bytes(stream.data) {
            push_pdf_stream_candidate(&mut candidates, decoded.clone());
            seeds.push(decoded);
        }
    }

    if stream.is_flate {
        for seed in seeds {
            if let Some(decoded) =
                inflate_pdf_stream_zlib(&seed).or_else(|| inflate_pdf_stream_deflate(&seed))
            {
                push_pdf_stream_candidate(&mut candidates, decoded);
            }
        }
    }

    candidates
}

fn push_pdf_stream_candidate(candidates: &mut Vec<PdfDecodedStream>, data: Vec<u8>) {
    if candidates.iter().any(|candidate| candidate.data == data) {
        return;
    }
    candidates.push(PdfDecodedStream { data });
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn inflate_pdf_stream_zlib(data: &[u8]) -> Option<Vec<u8>> {
    const MAX_INFLATED_BYTES: usize = 5 * 1024 * 1024;

    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    match decoder
        .by_ref()
        .take(MAX_INFLATED_BYTES as u64 + 1)
        .read_to_end(&mut output)
    {
        Ok(_) if output.len() <= MAX_INFLATED_BYTES => Some(output),
        _ => None,
    }
}

fn inflate_pdf_stream_deflate(data: &[u8]) -> Option<Vec<u8>> {
    const MAX_INFLATED_BYTES: usize = 5 * 1024 * 1024;

    let mut decoder = DeflateDecoder::new(data);
    let mut output = Vec::new();
    match decoder
        .by_ref()
        .take(MAX_INFLATED_BYTES as u64 + 1)
        .read_to_end(&mut output)
    {
        Ok(_) if output.len() <= MAX_INFLATED_BYTES => Some(output),
        _ => None,
    }
}

fn decode_ascii_hex_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let mut hex_chars = Vec::new();
    for byte in data.iter().copied() {
        if byte == b'>' {
            break;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        if !byte.is_ascii_hexdigit() {
            return None;
        }
        hex_chars.push(byte);
    }
    if hex_chars.is_empty() {
        return None;
    }
    if hex_chars.len() % 2 == 1 {
        hex_chars.push(b'0');
    }

    let decoded: Vec<u8> = hex_chars
        .chunks(2)
        .filter_map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect();
    (!decoded.is_empty()).then_some(decoded)
}

fn decode_ascii85_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut group = Vec::with_capacity(5);
    let mut started = false;
    let mut i = 0usize;

    while i < data.len() {
        let byte = data[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if byte == b'<' && data.get(i + 1) == Some(&b'~') {
            started = true;
            i += 2;
            continue;
        }
        if byte == b'~' && data.get(i + 1) == Some(&b'>') {
            break;
        }
        if byte == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            started = true;
            i += 1;
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return None;
        }
        started = true;
        group.push(byte);
        if group.len() == 5 {
            decode_ascii85_group(&group, 5, &mut out)?;
            group.clear();
        }
        i += 1;
    }

    if !group.is_empty() {
        let original_len = group.len();
        while group.len() < 5 {
            group.push(b'u');
        }
        decode_ascii85_group(&group, original_len, &mut out)?;
    }

    (started && !out.is_empty()).then_some(out)
}

fn decode_ascii85_group(group: &[u8], original_len: usize, out: &mut Vec<u8>) -> Option<()> {
    let mut value = 0u32;
    for byte in group {
        value = value
            .checked_mul(85)?
            .checked_add(u32::from(byte.checked_sub(b'!')?))?;
    }
    let bytes = value.to_be_bytes();
    let take = if original_len == 5 {
        4
    } else {
        original_len - 1
    };
    out.extend_from_slice(&bytes[..take]);
    Some(())
}

fn extract_text_ops_from_latin1(pdf: &str, out: &mut Vec<String>) {
    let mut rest: &str = pdf;

    while let Some(bt_start) = rest.find("BT") {
        rest = &rest[bt_start + 2..];
        let Some(et_end) = rest.find("ET") else {
            break;
        };
        let block = &rest[..et_end];
        extract_pdf_string_operands(block, out);
        rest = &rest[et_end + 2..];
    }
}

fn extract_pdf_string_operands(block: &str, out: &mut Vec<String>) {
    let bytes = block.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let (raw, next) = read_pdf_literal_string(bytes, i + 1);
            i = next;
            let decoded = decode_pdf_literal_string(&raw);
            if decoded.trim().is_empty() {
                continue;
            }

            out.push(decoded.trim().to_string());
            continue;
        }

        if bytes[i] == b'<' && bytes.get(i + 1) != Some(&b'<') {
            let (raw_hex, next) = read_pdf_hex_string(bytes, i + 1);
            i = next;
            let decoded = decode_pdf_hex_string(&raw_hex);
            if decoded.trim().is_empty() {
                continue;
            }

            out.push(decoded.trim().to_string());
            continue;
        }

        if bytes[i] != b'\'' && bytes[i] != b'"' {
            i += 1;
            continue;
        }

        i += 1;
    }
}

fn read_pdf_literal_string(bytes: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut raw = Vec::new();
    let mut depth = 1usize;
    let mut i = start;

    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\\' {
            raw.push(byte);
            if let Some(next) = bytes.get(i + 1) {
                raw.push(*next);
                i += 2;
                continue;
            }
        } else if byte == b'(' {
            depth += 1;
            raw.push(byte);
        } else if byte == b')' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return (raw, i + 1);
            }
            raw.push(byte);
        } else {
            raw.push(byte);
        }
        i += 1;
    }

    (raw, i)
}

fn read_pdf_hex_string(bytes: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut raw = Vec::new();
    let mut i = start;

    while i < bytes.len() {
        if bytes[i] == b'>' {
            return (raw, i + 1);
        }
        raw.push(bytes[i]);
        i += 1;
    }

    (raw, i)
}

fn decode_pdf_literal_string(raw: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0;

    while i < raw.len() {
        if raw[i] != b'\\' {
            out.push(raw[i]);
            i += 1;
            continue;
        }

        let Some(&next) = raw.get(i + 1) else {
            break;
        };
        match next {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'(' | b')' | b'\\' => out.push(next),
            b'0'..=b'7' => {
                let mut value = 0u8;
                let mut consumed = 0usize;
                for j in i + 1..raw.len().min(i + 4) {
                    if !(b'0'..=b'7').contains(&raw[j]) {
                        break;
                    }
                    value = value.saturating_mul(8).saturating_add(raw[j] - b'0');
                    consumed += 1;
                }
                out.push(value);
                i += consumed + 1;
                continue;
            }
            b'\n' | b'\r' => {}
            other => out.push(other),
        }
        i += 2;
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn decode_pdf_hex_string(raw: &[u8]) -> String {
    let mut hex_chars: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|byte| byte.is_ascii_hexdigit())
        .collect();
    if hex_chars.len() % 2 == 1 {
        hex_chars.push(b'0');
    }

    let bytes: Vec<u8> = hex_chars
        .chunks(2)
        .filter_map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect();

    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if looks_like_utf16be_ascii(&bytes) {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

fn looks_like_utf16be_ascii(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes.len() % 2 == 0
        && bytes
            .chunks_exact(2)
            .filter(|pair| pair[0] == 0 && pair[1].is_ascii())
            .count()
            * 2
            >= bytes.len() / 2
}

fn dedupe_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();

    for item in items {
        let normalized = item.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        deduped.push(item);
    }

    deduped
}

/// 从 media_type 获取图片格式
fn get_image_format(media_type: &str) -> Option<String> {
    match media_type {
        "image/jpeg" => Some("jpeg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        _ => None,
    }
}

/// 提取工具结果内容
fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            parts.join("\n")
        }
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// 验证并过滤 tool_use/tool_result 配对
///
/// 收集所有 tool_use_id，验证 tool_result 是否匹配
/// 静默跳过孤立的 tool_use 和 tool_result，输出警告日志
///
/// # Arguments
/// * `history` - 历史消息引用
/// * `tool_results` - 当前消息中的 tool_result 列表
///
/// # Returns
/// 元组：(经过验证和过滤后的 tool_result 列表, 孤立的 tool_use_id 集合)
fn validate_tool_pairing(
    history: &[Message],
    tool_results: &[ToolResult],
) -> (Vec<ToolResult>, std::collections::HashSet<String>) {
    use std::collections::HashSet;

    // 1. 收集所有历史中的 tool_use_id
    let mut all_tool_use_ids: HashSet<String> = HashSet::new();
    // 2. 收集历史中已经有 tool_result 的 tool_use_id
    let mut history_tool_result_ids: HashSet<String> = HashSet::new();

    for msg in history {
        match msg {
            Message::Assistant(assistant_msg) => {
                if let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        all_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
            }
            Message::User(user_msg) => {
                // 收集历史 user 消息中的 tool_results
                for result in &user_msg
                    .user_input_message
                    .user_input_message_context
                    .tool_results
                {
                    history_tool_result_ids.insert(result.tool_use_id.clone());
                }
            }
        }
    }

    // 3. 计算真正未配对的 tool_use_ids（排除历史中已配对的）
    let mut unpaired_tool_use_ids: HashSet<String> = all_tool_use_ids
        .difference(&history_tool_result_ids)
        .cloned()
        .collect();

    // 4. 过滤并验证当前消息的 tool_results
    let mut filtered_results = Vec::new();

    for result in tool_results {
        if unpaired_tool_use_ids.contains(&result.tool_use_id) {
            // 配对成功
            filtered_results.push(result.clone());
            unpaired_tool_use_ids.remove(&result.tool_use_id);
        } else if all_tool_use_ids.contains(&result.tool_use_id) {
            // tool_use 存在但已经在历史中配对过了，这是重复的 tool_result
            tracing::warn!(
                "跳过重复的 tool_result：该 tool_use 已在历史中配对，tool_use_id={}",
                result.tool_use_id
            );
        } else {
            // 孤立 tool_result - 找不到对应的 tool_use
            tracing::warn!(
                "跳过孤立的 tool_result：找不到对应的 tool_use，tool_use_id={}",
                result.tool_use_id
            );
        }
    }

    // 5. 检测真正孤立的 tool_use（有 tool_use 但在历史和当前消息中都没有 tool_result）
    for orphaned_id in &unpaired_tool_use_ids {
        tracing::warn!(
            "检测到孤立的 tool_use：找不到对应的 tool_result，将从历史中移除，tool_use_id={}",
            orphaned_id
        );
    }

    (filtered_results, unpaired_tool_use_ids)
}

/// 从历史消息中移除孤立的 tool_use
///
/// Kiro API 要求每个 tool_use 必须有对应的 tool_result，否则返回 400 Bad Request。
/// 此函数遍历历史中的 assistant 消息，移除没有对应 tool_result 的 tool_use。
///
/// # Arguments
/// * `history` - 可变的历史消息列表
/// * `orphaned_ids` - 需要移除的孤立 tool_use_id 集合
fn remove_orphaned_tool_uses(
    history: &mut [Message],
    orphaned_ids: &std::collections::HashSet<String>,
) {
    if orphaned_ids.is_empty() {
        return;
    }

    for msg in history.iter_mut() {
        if let Message::Assistant(assistant_msg) = msg {
            if let Some(ref mut tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                let original_len = tool_uses.len();
                tool_uses.retain(|tu| !orphaned_ids.contains(&tu.tool_use_id));

                // 如果移除后为空，设置为 None
                if tool_uses.is_empty() {
                    assistant_msg.assistant_response_message.tool_uses = None;
                } else if tool_uses.len() != original_len {
                    tracing::debug!(
                        "从 assistant 消息中移除了 {} 个孤立的 tool_use",
                        original_len - tool_uses.len()
                    );
                }
            }
        }
    }
}

/// Kiro API 工具名称最大长度限制
const TOOL_NAME_MAX_LEN: usize = 63;

/// 生成确定性短名称：截断前缀 + "_" + 8 位 SHA256 hex
fn shorten_tool_name(name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    let hash_suffix = &hash_hex[..8];
    // 54 prefix + 1 underscore + 8 hash = 63
    let prefix_max = TOOL_NAME_MAX_LEN - 1 - 8;
    let prefix = match name.char_indices().nth(prefix_max) {
        Some((idx, _)) => &name[..idx],
        None => name,
    };
    format!("{}_{}", prefix, hash_suffix)
}

/// 如果名称超长则缩短，并记录映射（short → original）
fn map_tool_name(name: &str, tool_name_map: &mut HashMap<String, String>) -> String {
    if name.len() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }
    let short = shorten_tool_name(name);
    tool_name_map.insert(short.clone(), name.to_string());
    short
}

/// 转换工具定义
fn convert_tools(
    tools: &Option<Vec<super::types::Tool>>,
    tool_name_map: &mut HashMap<String, String>,
    options: ConversionOptions,
) -> Vec<Tool> {
    let Some(tools) = tools else {
        return Vec::new();
    };

    tools
        .iter()
        .map(|t| {
            let mut description = t.description.clone();

            // 对 Write/Edit 工具追加自定义描述后缀
            if !options.clean_probe_mode {
                let suffix = match t.name.as_str() {
                    "Write" => WRITE_TOOL_DESCRIPTION_SUFFIX,
                    "Edit" => EDIT_TOOL_DESCRIPTION_SUFFIX,
                    _ => "",
                };
                if !suffix.is_empty() {
                    description.push('\n');
                    description.push_str(suffix);
                }
            }

            // 限制描述长度为 10000 字符（安全截断 UTF-8，单次遍历）
            let description = match description.char_indices().nth(10000) {
                Some((idx, _)) => description[..idx].to_string(),
                None => description,
            };

            Tool {
                tool_specification: ToolSpecification {
                    name: map_tool_name(&t.name, tool_name_map),
                    description,
                    input_schema: InputSchema::from_json(normalize_json_schema(serde_json::json!(
                        t.input_schema
                    ))),
                },
            }
        })
        .collect()
}

/// 生成thinking标签前缀
fn generate_thinking_prefix(req: &MessagesRequest) -> Option<String> {
    if let Some(t) = &req.thinking {
        if t.thinking_type == "enabled" {
            return Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{}</max_thinking_length>",
                t.budget_tokens
            ));
        } else if t.thinking_type == "adaptive" {
            let effort = req
                .output_config
                .as_ref()
                .map(|c| c.effort.as_str())
                .unwrap_or("high");
            return Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{}</thinking_effort>",
                effort
            ));
        }
    }
    None
}

/// 检查内容是否已包含thinking标签
fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

fn apply_current_message_thinking_prefix(req: &MessagesRequest, content: String) -> String {
    let Some(prefix) = generate_thinking_prefix(req) else {
        return content;
    };

    if has_thinking_tags(&content) {
        return content;
    }

    if content.is_empty() {
        prefix
    } else {
        format!("{}\n{}", prefix, content)
    }
}

fn apply_current_message_prefixes(
    req: &MessagesRequest,
    content: String,
    options: ConversionOptions,
) -> String {
    if !options.clean_probe_mode {
        return apply_current_message_thinking_prefix(req, content);
    }

    let mut parts = Vec::new();

    if let Some(prefix) = generate_thinking_prefix(req) {
        if !has_thinking_tags(&content) {
            parts.push(prefix);
        }
    }

    if let Some(hint) = structured_output_hint(req) {
        if !content.contains("Respond with valid JSON only") {
            parts.push(hint);
        }
    }

    if !content.is_empty() {
        parts.push(content);
    }

    parts.join("\n")
}

fn structured_output_hint(req: &MessagesRequest) -> Option<String> {
    let format = req
        .output_config
        .as_ref()
        .and_then(|config| config.format.as_ref())
        .or(req.response_format.as_ref())?;

    structured_output_hint_from_format(format)
}

fn structured_output_hint_from_format(format: &StructuredOutputFormat) -> Option<String> {
    let format_type = format.format_type.as_str();
    match format_type {
        "json_object" => Some(
            "Respond with valid JSON only. Do not include markdown, code fences, comments, or explanatory prose. The entire assistant response must be parseable as one JSON value."
                .to_string(),
        ),
        "json_schema" => {
            let schema = format
                .schema
                .as_ref()
                .or_else(|| format.json_schema.as_ref().and_then(|schema| schema.schema.as_ref()))?;
            let name = format
                .name
                .as_deref()
                .or_else(|| format.json_schema.as_ref().and_then(|schema| schema.name.as_deref()))
                .unwrap_or("response");
            let strict = format
                .strict
                .or_else(|| format.json_schema.as_ref().and_then(|schema| schema.strict))
                .unwrap_or(true);
            let schema_text = serde_json::to_string(schema).ok()?;

            Some(format!(
                "Respond with valid JSON only. Do not include markdown, code fences, comments, or explanatory prose. The entire assistant response must be parseable as one JSON value. Match the JSON Schema named `{}`{}:\n{}",
                name,
                if strict { " strictly" } else { "" },
                schema_text
            ))
        }
        _ => None,
    }
}

/// 构建历史消息
///
/// # Arguments
/// * `req` - 原始请求，用于读取 `system`、`thinking` 等配置字段
/// * `messages` - 经过 prefill 预处理的消息切片，末尾必定是 user 消息。
///   注意：该切片与 `req.messages` 可能不同（prefill 时会截断末尾的 assistant 消息），
///   调用方应始终使用此参数而非 `req.messages`。
/// * `model_id` - 已映射的 Kiro 模型 ID
fn build_history(
    req: &MessagesRequest,
    messages: &[super::types::Message],
    model_id: &str,
    tool_name_map: &mut HashMap<String, String>,
    options: ConversionOptions,
) -> Result<Vec<Message>, ConversionError> {
    let mut history = Vec::new();

    // 生成thinking前缀（如果需要）
    let thinking_prefix = generate_thinking_prefix(req);
    let output_hint = structured_output_hint(req);

    // 1. 处理系统消息
    if let Some(ref system) = req.system {
        let system_content: String = system
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join("\n");

        if !system_content.is_empty() {
            let mut system_content = system_content;
            if !options.clean_probe_mode {
                system_content.push('\n');
                system_content.push_str(SYSTEM_CHUNKED_POLICY);
            }
            if !options.clean_probe_mode {
                if let Some(ref hint) = output_hint {
                    system_content.push('\n');
                    system_content.push_str(hint);
                }
            }

            // 注入thinking标签到系统消息最前面（如果需要且不存在）
            let final_content = if !options.clean_probe_mode {
                if let Some(ref prefix) = thinking_prefix {
                    if !has_thinking_tags(&system_content) {
                        format!("{}\n{}", prefix, system_content)
                    } else {
                        system_content
                    }
                } else {
                    system_content
                }
            } else {
                system_content
            };

            // 系统消息作为 user + assistant 配对
            let user_msg = HistoryUserMessage::new(final_content, model_id);
            history.push(Message::User(user_msg));

            if !options.clean_probe_mode {
                let assistant_msg =
                    HistoryAssistantMessage::new("I will follow these instructions.");
                history.push(Message::Assistant(assistant_msg));
            }
        }
    } else if !options.clean_probe_mode && (thinking_prefix.is_some() || output_hint.is_some()) {
        // 没有系统消息但有 thinking/structured-output 配置，插入新的系统消息
        let mut parts = Vec::new();
        if let Some(ref prefix) = thinking_prefix {
            parts.push(prefix.clone());
        }
        if let Some(ref hint) = output_hint {
            parts.push(hint.clone());
        }
        let user_msg = HistoryUserMessage::new(parts.join("\n"), model_id);
        history.push(Message::User(user_msg));

        let assistant_msg = HistoryAssistantMessage::new("I will follow these instructions.");
        history.push(Message::Assistant(assistant_msg));
    }

    // 2. 处理常规消息历史
    // 最后一条消息作为 currentMessage，不加入历史
    // 经过 prefill 预处理后，messages 末尾必定是 user，故直接截掉最后一条即可
    let history_end_index = messages.len().saturating_sub(1);

    // 收集并配对消息
    let mut user_buffer: Vec<&super::types::Message> = Vec::new();
    let mut assistant_buffer: Vec<&super::types::Message> = Vec::new();

    for msg in messages.iter().take(history_end_index) {
        if msg.role == "user" {
            // 先处理累积的 assistant 消息
            if !assistant_buffer.is_empty() {
                let merged = merge_assistant_messages(&assistant_buffer, tool_name_map)?;
                history.push(Message::Assistant(merged));
                assistant_buffer.clear();
            }
            user_buffer.push(msg);
        } else if msg.role == "assistant" {
            // 先处理累积的 user 消息
            if !user_buffer.is_empty() {
                let merged_user = merge_user_messages(&user_buffer, model_id)?;
                history.push(Message::User(merged_user));
                user_buffer.clear();
            }
            // 累积 assistant 消息（支持连续多条）
            assistant_buffer.push(msg);
        }
    }

    // 处理末尾累积的 assistant 消息
    if !assistant_buffer.is_empty() {
        let merged = merge_assistant_messages(&assistant_buffer, tool_name_map)?;
        history.push(Message::Assistant(merged));
    }

    // 处理结尾的孤立 user 消息
    if !user_buffer.is_empty() {
        let merged_user = merge_user_messages(&user_buffer, model_id)?;
        history.push(Message::User(merged_user));

        // 自动配对一个 "OK" 的 assistant 响应
        let auto_assistant = HistoryAssistantMessage::new("OK");
        history.push(Message::Assistant(auto_assistant));
    }

    Ok(history)
}

/// 合并多个 user 消息
fn merge_user_messages(
    messages: &[&super::types::Message],
    model_id: &str,
) -> Result<HistoryUserMessage, ConversionError> {
    let mut content_parts = Vec::new();
    let mut all_images = Vec::new();
    let mut all_tool_results = Vec::new();

    for msg in messages {
        let processed = process_message_content(&msg.content)?;
        if !processed.text.is_empty() {
            content_parts.push(processed.text);
        }
        all_images.extend(processed.images);
        all_tool_results.extend(processed.tool_results);
    }

    let content = content_parts.join("\n");
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let mut user_msg = UserMessage::new(&content, model_id);

    if !all_images.is_empty() {
        user_msg = user_msg.with_images(all_images);
    }

    if !all_tool_results.is_empty() {
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(all_tool_results);
        user_msg = user_msg.with_context(ctx);
    }

    Ok(HistoryUserMessage {
        user_input_message: user_msg,
    })
}

/// 转换 assistant 消息
fn convert_assistant_message(
    msg: &super::types::Message,
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    let mut thinking_content = String::new();
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();

    match &msg.content {
        serde_json::Value::String(s) => {
            text_content = s.clone();
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "thinking" => {
                            if let Some(thinking) = block.thinking {
                                thinking_content.push_str(&thinking);
                            }
                        }
                        "text" => {
                            if let Some(text) = block.text {
                                text_content.push_str(&text);
                            }
                        }
                        "tool_use" => {
                            if let (Some(id), Some(name)) = (block.id, block.name) {
                                let input = block.input.unwrap_or(serde_json::json!({}));
                                let mapped_name = map_tool_name(&name, tool_name_map);
                                tool_uses
                                    .push(ToolUseEntry::new(id, mapped_name).with_input(input));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    // 组合 thinking 和 text 内容
    // 格式: <thinking>思考内容</thinking>\n\ntext内容
    // 注意: Kiro API 要求 content 字段不能为空，当只有 tool_use 时需要占位符
    let final_content = if !thinking_content.is_empty() {
        if !text_content.is_empty() {
            format!(
                "<thinking>{}</thinking>\n\n{}",
                thinking_content, text_content
            )
        } else {
            format!("<thinking>{}</thinking>", thinking_content)
        }
    } else if text_content.is_empty() && !tool_uses.is_empty() {
        " ".to_string()
    } else {
        text_content
    };

    let mut assistant = AssistantMessage::new(final_content);
    if !tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(tool_uses);
    }

    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

/// 合并多个连续的 assistant 消息为一条
/// 用于处理网络不稳定时产生的连续 assistant 消息（Issue #79）
fn merge_assistant_messages(
    messages: &[&super::types::Message],
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    assert!(!messages.is_empty());
    if messages.len() == 1 {
        return convert_assistant_message(messages[0], tool_name_map);
    }

    let mut all_tool_uses: Vec<ToolUseEntry> = Vec::new();
    let mut content_parts: Vec<String> = Vec::new();

    for msg in messages {
        let converted = convert_assistant_message(msg, tool_name_map)?;
        let am = converted.assistant_response_message;
        if !am.content.trim().is_empty() {
            content_parts.push(am.content);
        }
        if let Some(tus) = am.tool_uses {
            all_tool_uses.extend(tus);
        }
    }

    let content = if content_parts.is_empty() && !all_tool_uses.is_empty() {
        " ".to_string()
    } else {
        content_parts.join("\n\n")
    };

    let mut assistant = AssistantMessage::new(content);
    if !all_tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(all_tool_uses);
    }
    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_request(content: serde_json::Value) -> MessagesRequest {
        use super::super::types::Message as AnthropicMessage;

        MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content,
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: None,
        }
    }

    #[test]
    fn test_thinking_prefix_is_added_to_current_message() {
        let mut req = minimal_request(serde_json::json!("answer briefly"));
        req.model = "claude-opus-4-7".to_string();
        req.thinking = Some(super::super::types::Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 20000,
        });
        req.output_config = Some(super::super::types::OutputConfig {
            effort: "high".to_string(),
            format: None,
        });

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(current.content.starts_with(
            "<thinking_mode>adaptive</thinking_mode><thinking_effort>high</thinking_effort>"
        ));
        assert!(current.content.contains("answer briefly"));
    }

    #[test]
    fn test_map_model_sonnet() {
        assert!(
            map_model("claude-sonnet-4-20250514")
                .unwrap()
                .contains("sonnet")
        );
        assert!(
            map_model("claude-3-5-sonnet-20241022")
                .unwrap()
                .contains("sonnet")
        );
    }

    #[test]
    fn test_map_model_opus() {
        assert!(
            map_model("claude-opus-4-20250514")
                .unwrap()
                .contains("opus")
        );
    }

    #[test]
    fn test_map_model_haiku() {
        assert!(
            map_model("claude-haiku-4-20250514")
                .unwrap()
                .contains("haiku")
        );
    }

    #[test]
    fn test_map_model_unsupported() {
        assert!(map_model("gpt-4").is_none());
    }

    #[test]
    fn test_map_model_thinking_suffix_sonnet() {
        // thinking 后缀不应影响 sonnet 模型映射
        let result = map_model("claude-sonnet-4-5-20250929-thinking");
        assert_eq!(result, Some("claude-sonnet-4.5".to_string()));
    }

    #[test]
    fn test_map_model_thinking_suffix_opus_4_5() {
        // thinking 后缀不应影响 opus 4.5 模型映射
        let result = map_model("claude-opus-4-5-20251101-thinking");
        assert_eq!(result, Some("claude-opus-4.5".to_string()));
    }

    #[test]
    fn test_map_model_thinking_suffix_opus_4_6() {
        // thinking 后缀不应影响 opus 4.6 模型映射
        let result = map_model("claude-opus-4-6-thinking");
        assert_eq!(result, Some("claude-opus-4.6".to_string()));
    }

    #[test]
    fn test_map_model_thinking_suffix_opus_4_7() {
        // thinking 后缀不应影响 opus 4.7 模型映射
        let result = map_model("claude-opus-4-7-thinking");
        assert_eq!(result, Some("claude-opus-4.7".to_string()));
    }

    #[test]
    fn test_opus_4_6_and_4_7_conversion_only_differs_by_model_id() {
        use super::super::types::{
            Message as AnthropicMessage, Metadata, SystemMessage, Tool as AnthropicTool,
        };

        let mut schema = std::collections::HashMap::new();
        schema.insert("type".to_string(), serde_json::json!("object"));
        schema.insert(
            "properties".to_string(),
            serde_json::json!({"path": {"type": "string"}}),
        );

        let build_req = |model: &str| MessagesRequest {
            model: model.to_string(),
            max_tokens: 256,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("first turn"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!("first answer"),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "text", "text": "same prompt"}
                    ]),
                },
            ],
            stream: true,
            system: Some(vec![SystemMessage {
                text: "same system".to_string(),
                cache_control: None,
            }]),
            tools: Some(vec![AnthropicTool {
                name: "Read".to_string(),
                description: "Read a file".to_string(),
                input_schema: schema.clone(),
                tool_type: None,
                max_uses: None,
                cache_control: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: Some(Metadata {
                user_id: Some(
                    r#"{"session_id":"8bb5523b-ec7c-4540-a9ca-beb6d79f1552"}"#.to_string(),
                ),
            }),
        };

        let mut opus_46 = serde_json::to_value(
            convert_request(&build_req("claude-opus-4-6"))
                .unwrap()
                .conversation_state,
        )
        .unwrap();
        let mut opus_47 = serde_json::to_value(
            convert_request(&build_req("claude-opus-4-7"))
                .unwrap()
                .conversation_state,
        )
        .unwrap();

        assert_eq!(
            opus_46
                .pointer("/currentMessage/userInputMessage/modelId")
                .and_then(|v| v.as_str()),
            Some("claude-opus-4.6")
        );
        assert_eq!(
            opus_47
                .pointer("/currentMessage/userInputMessage/modelId")
                .and_then(|v| v.as_str()),
            Some("claude-opus-4.7")
        );

        opus_46["currentMessage"]["userInputMessage"]["modelId"] =
            serde_json::json!("MODEL_PLACEHOLDER");
        opus_47["currentMessage"]["userInputMessage"]["modelId"] =
            serde_json::json!("MODEL_PLACEHOLDER");
        opus_46["agentContinuationId"] = serde_json::json!("AGENT_CONTINUATION_PLACEHOLDER");
        opus_47["agentContinuationId"] = serde_json::json!("AGENT_CONTINUATION_PLACEHOLDER");

        if let Some(history) = opus_46.get_mut("history").and_then(|v| v.as_array_mut()) {
            for item in history {
                if let Some(user) = item.get_mut("userInputMessage") {
                    user["modelId"] = serde_json::json!("MODEL_PLACEHOLDER");
                }
            }
        }
        if let Some(history) = opus_47.get_mut("history").and_then(|v| v.as_array_mut()) {
            for item in history {
                if let Some(user) = item.get_mut("userInputMessage") {
                    user["modelId"] = serde_json::json!("MODEL_PLACEHOLDER");
                }
            }
        }

        assert_eq!(opus_46, opus_47);
    }

    #[test]
    fn test_map_model_thinking_suffix_haiku() {
        // thinking 后缀不应影响 haiku 模型映射
        let result = map_model("claude-haiku-4-5-20251001-thinking");
        assert_eq!(result, Some("claude-haiku-4.5".to_string()));
    }

    #[test]
    fn test_determine_chat_trigger_type() {
        // 无工具时返回 MANUAL
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };
        assert_eq!(determine_chat_trigger_type(&req), "MANUAL");
    }

    #[test]
    fn test_pdf_document_block_is_preserved_and_extracted_into_text() {
        let pdf = "%PDF-1.4\n1 0 obj\n<< /Type /Page >>\nendobj\n2 0 obj\n<< /Length 44 >>\nstream\nBT\n/F1 12 Tf\n72 720 Td\n(Invoice total 42) Tj\nET\nendstream\nendobj\n%%EOF";
        let encoded = STANDARD.encode(pdf.as_bytes());
        let req = minimal_request(serde_json::json!([
            {"type": "text", "text": "Read the attached PDF."},
            {
                "type": "document",
                "title": "invoice.pdf",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": encoded
                }
            }
        ]));

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(current.content.contains("Read the attached PDF."));
        assert!(
            current.content.contains("Invoice total 42"),
            "extracted PDF text should be visible to upstream model: {}",
            current.content
        );
    }

    #[test]
    fn test_pdf_fallback_extracts_flate_stream_text_without_xref() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let stream_text = b"BT\n/F1 12 Tf\n72 720 Td\n(Compressed invoice total 99) Tj\n<0050004400460020006800650078> Tj\nET";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream_text).unwrap();
        let compressed = encoder.finish().unwrap();
        let pdf = [
            b"%PDF-1.4\n1 0 obj\n<< /Type /Page >>\nendobj\n2 0 obj\n<< /Filter /FlateDecode /Length ".as_slice(),
            compressed.len().to_string().as_bytes(),
            b" >>\nstream\n",
            compressed.as_slice(),
            b"\nendstream\nendobj\n%%EOF",
        ]
        .concat();
        let encoded = STANDARD.encode(pdf);
        let req = minimal_request(serde_json::json!([
            {"type": "text", "text": "Read the attached PDF."},
            {
                "type": "document",
                "title": "compressed.pdf",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": encoded
                }
            }
        ]));

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(
            current.content.contains("Compressed invoice total 99"),
            "compressed PDF stream text should be visible: {}",
            current.content
        );
        assert!(
            current.content.contains("PDF hex"),
            "hex PDF text should be decoded: {}",
            current.content
        );
    }

    #[test]
    fn test_pdf_fallback_extracts_raw_deflate_stream_text_without_xref() {
        use flate2::{Compression, write::DeflateEncoder};
        use std::io::Write;

        let stream_text = b"BT\n/F1 12 Tf\n72 720 Td\n(Raw deflate invoice 123) Tj\nET";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream_text).unwrap();
        let compressed = encoder.finish().unwrap();
        let pdf = [
            b"%PDF-1.4\n1 0 obj\n<< /Type /Page >>\nendobj\n2 0 obj\n<< /Filter /FlateDecode /Length ".as_slice(),
            compressed.len().to_string().as_bytes(),
            b" >>\nstream\n",
            compressed.as_slice(),
            b"\nendstream\nendobj\n%%EOF",
        ]
        .concat();
        let encoded = STANDARD.encode(pdf);
        let req = minimal_request(serde_json::json!([
            {"type": "text", "text": "Read the attached PDF."},
            {
                "type": "document",
                "title": "raw-deflate.pdf",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": encoded
                }
            }
        ]));

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(
            current.content.contains("Raw deflate invoice 123"),
            "raw deflate stream text should be visible: {}",
            current.content
        );
    }

    #[test]
    fn test_pdf_fallback_extracts_ascii_hex_flate_stream_text_without_xref() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let stream_text = b"BT\n/F1 12 Tf\n72 720 Td\n(ASCIIHex flate invoice 456) Tj\nET";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream_text).unwrap();
        let compressed = encoder.finish().unwrap();
        let hex = compressed
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
            .into_bytes();
        let pdf = [
            b"%PDF-1.4\n1 0 obj\n<< /Type /Page >>\nendobj\n2 0 obj\n<< /Filter [/ASCIIHexDecode /FlateDecode] /Length ".as_slice(),
            hex.len().to_string().as_bytes(),
            b" >>\nstream\n",
            hex.as_slice(),
            b">\nendstream\nendobj\n%%EOF",
        ]
        .concat();
        let encoded = STANDARD.encode(pdf);
        let req = minimal_request(serde_json::json!([
            {"type": "text", "text": "Read the attached PDF."},
            {
                "type": "document",
                "title": "asciihex-flate.pdf",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": encoded
                }
            }
        ]));

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(
            current.content.contains("ASCIIHex flate invoice 456"),
            "ASCIIHex+Flate stream text should be visible: {}",
            current.content
        );
    }

    #[test]
    fn test_pdf_fallback_extracts_cctest_broken_xref_text() {
        let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 150 50] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>
endobj
4 0 obj
<< /Length 38 >>
stream
BT /F1 14 Tf 10 20 Td (hvoyqsyz) Tj ET
endstream
endobj
5 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
xref
0 6
0000000000 65535 f 
trailer
<< /Size 6 /Root 1 0 R >>
startxref
0
%%EOF";
        let encoded = STANDARD.encode(pdf);
        let req = minimal_request(serde_json::json!([
            {"type": "text", "text": "What text is in the attached PDF?"},
            {
                "type": "document",
                "title": "document.pdf",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": encoded
                }
            }
        ]));

        let state = convert_request(&req).unwrap().conversation_state;
        let current = &state.current_message.user_input_message;

        assert!(
            current.content.contains("Page count: 1"),
            "page count should not count /Pages as /Page: {}",
            current.content
        );
        assert!(
            current.content.contains("Extracted text:\nhvoyqsyz"),
            "broken xref PDF text should be injected clearly: {}",
            current.content
        );
    }

    #[test]
    fn test_pdf_short_primary_text_prefers_substantial_fallback() {
        let selected = select_pdf_text("Page 1/2", "Invoice total 42\nDue today");

        assert_eq!(selected.source, "fallback");
        assert_eq!(selected.primary_chars, 8);
        assert!(selected.text.contains("Invoice total 42"));
    }

    #[test]
    fn test_structured_output_config_injects_json_schema_hint() {
        use super::super::types::{JsonSchemaFormat, OutputConfig, StructuredOutputFormat};

        let mut req = minimal_request(serde_json::json!("Return the result."));
        req.output_config = Some(OutputConfig {
            effort: "high".to_string(),
            format: Some(StructuredOutputFormat {
                format_type: "json_schema".to_string(),
                name: Some("answer".to_string()),
                schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"}
                    },
                    "required": ["answer"],
                    "additionalProperties": false
                })),
                json_schema: None::<JsonSchemaFormat>,
                strict: Some(true),
            }),
        });

        let state = convert_request(&req).unwrap().conversation_state;
        let first_history = state.history.first().expect("structured hint history");
        let Message::User(user_msg) = first_history else {
            panic!("first history item should be user instructions");
        };

        let content = &user_msg.user_input_message.content;
        assert!(content.contains("Respond with valid JSON only"));
        assert!(content.contains("JSON Schema named `answer` strictly"));
        assert!(content.contains("\"additionalProperties\":false"));
    }

    #[test]
    fn test_clean_probe_mode_keeps_thinking_and_schema_on_current_message_without_synthetic_history()
     {
        use super::super::types::{OutputConfig, StructuredOutputFormat, SystemMessage, Thinking};

        let mut req = minimal_request(serde_json::json!("Return the result."));
        req.model = "claude-opus-4-7".to_string();
        req.system = Some(vec![SystemMessage {
            text: "System instruction".to_string(),
            cache_control: None,
        }]);
        req.thinking = Some(Thinking {
            thinking_type: "enabled".to_string(),
            budget_tokens: 20000,
        });
        req.output_config = Some(OutputConfig {
            effort: "high".to_string(),
            format: Some(StructuredOutputFormat {
                format_type: "json_schema".to_string(),
                name: Some("answer".to_string()),
                schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"}
                    },
                    "required": ["answer"],
                    "additionalProperties": false
                })),
                json_schema: None,
                strict: Some(true),
            }),
        });

        let state = convert_request_with_options(
            &req,
            ConversionOptions {
                clean_probe_mode: true,
            },
        )
        .unwrap()
        .conversation_state;

        assert_eq!(state.history.len(), 1);
        let Message::User(system_msg) = &state.history[0] else {
            panic!("clean probe should only preserve the real system message");
        };
        assert_eq!(system_msg.user_input_message.content, "System instruction");

        let current = &state.current_message.user_input_message.content;
        assert!(current.starts_with(
            "<thinking_mode>enabled</thinking_mode><max_thinking_length>20000</max_thinking_length>"
        ));
        assert!(current.contains("Respond with valid JSON only"));
        assert!(current.contains("Return the result."));
    }

    #[test]
    fn test_clean_probe_mode_skips_write_edit_description_suffixes() {
        use super::super::types::{Message as AnthropicMessage, Tool as AnthropicTool};

        let mut schema = std::collections::HashMap::new();
        schema.insert("type".to_string(), serde_json::json!("object"));
        schema.insert("properties".to_string(), serde_json::json!({}));

        let req = MessagesRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("test"),
            }],
            system: None,
            stream: false,
            tools: Some(vec![AnthropicTool {
                name: "Write".to_string(),
                description: "Write a file".to_string(),
                input_schema: schema,
                tool_type: None,
                max_uses: None,
                cache_control: None,
            }]),
            thinking: None,
            tool_choice: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let normal = convert_request(&req).unwrap();
        let clean = convert_request_with_options(
            &req,
            ConversionOptions {
                clean_probe_mode: true,
            },
        )
        .unwrap();

        let normal_description = &normal
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools[0]
            .tool_specification
            .description;
        let clean_description = &clean
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools[0]
            .tool_specification
            .description;

        assert!(normal_description.contains("IMPORTANT"));
        assert_eq!(clean_description, "Write a file");
    }

    #[test]
    fn test_openai_response_format_injects_json_hint() {
        use super::super::types::StructuredOutputFormat;

        let mut req = minimal_request(serde_json::json!("Return JSON."));
        req.response_format = Some(StructuredOutputFormat {
            format_type: "json_object".to_string(),
            name: None,
            schema: None,
            json_schema: None,
            strict: None,
        });

        let state = convert_request(&req).unwrap().conversation_state;
        let first_history = state.history.first().expect("json hint history");
        let Message::User(user_msg) = first_history else {
            panic!("first history item should be user instructions");
        };

        assert!(
            user_msg
                .user_input_message
                .content
                .contains("Respond with valid JSON only")
        );
    }

    #[test]
    fn test_collect_history_tool_names() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 创建包含工具使用的历史消息
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
            ToolUseEntry::new("tool-2", "write")
                .with_input(serde_json::json!({"path": "/out.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let tool_names = collect_history_tool_names(&history);
        assert_eq!(tool_names.len(), 2);
        assert!(tool_names.contains(&"read".to_string()));
        assert!(tool_names.contains(&"write".to_string()));
    }

    #[test]
    fn test_create_placeholder_tool() {
        let tool = create_placeholder_tool("my_custom_tool");

        assert_eq!(tool.tool_specification.name, "my_custom_tool");
        assert!(!tool.tool_specification.description.is_empty());

        // 验证 JSON 序列化正确
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"name\":\"my_custom_tool\""));
    }

    #[test]
    fn test_shorten_tool_name_deterministic() {
        let long_name =
            "mcp__some_very_long_server_name__some_very_long_tool_name_that_exceeds_limit";
        assert!(long_name.len() > TOOL_NAME_MAX_LEN);

        let short1 = shorten_tool_name(long_name);
        let short2 = shorten_tool_name(long_name);
        assert_eq!(short1, short2, "相同输入应产生相同的短名称");
        assert!(
            short1.len() <= TOOL_NAME_MAX_LEN,
            "短名称长度应 <= 63，实际 {}",
            short1.len()
        );
    }

    #[test]
    fn test_shorten_tool_name_uniqueness() {
        let name_a = "mcp__server_alpha__tool_name_that_is_very_long_and_exceeds_the_limit_a";
        let name_b = "mcp__server_alpha__tool_name_that_is_very_long_and_exceeds_the_limit_b";
        let short_a = shorten_tool_name(name_a);
        let short_b = shorten_tool_name(name_b);
        assert_ne!(short_a, short_b, "不同输入应产生不同的短名称");
    }

    #[test]
    fn test_map_tool_name_short_passthrough() {
        let mut map = HashMap::new();
        let result = map_tool_name("short_name", &mut map);
        assert_eq!(result, "short_name");
        assert!(map.is_empty(), "短名称不应产生映射");
    }

    #[test]
    fn test_map_tool_name_long_creates_mapping() {
        let mut map = HashMap::new();
        let long_name = "mcp__plugin_very_long_server_name__extremely_long_tool_name_exceeds_63";
        let result = map_tool_name(long_name, &mut map);
        assert!(result.len() <= TOOL_NAME_MAX_LEN);
        assert_eq!(map.get(&result), Some(&long_name.to_string()));
    }

    #[test]
    fn test_tool_name_mapping_in_convert_request() {
        use super::super::types::{Message as AnthropicMessage, Tool as AnthropicTool};

        let long_tool_name =
            "mcp__plugin_very_long_server_name__extremely_long_tool_name_exceeds_63";
        assert!(long_tool_name.len() > TOOL_NAME_MAX_LEN);

        let mut schema = std::collections::HashMap::new();
        schema.insert("type".to_string(), serde_json::json!("object"));
        schema.insert("properties".to_string(), serde_json::json!({}));

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("test"),
            }],
            system: None,
            stream: false,
            tools: Some(vec![AnthropicTool {
                name: long_tool_name.to_string(),
                description: "A test tool".to_string(),
                input_schema: schema,
                tool_type: None,
                max_uses: None,
                cache_control: None,
            }]),
            thinking: None,
            tool_choice: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let result = convert_request(&req).unwrap();

        // 应该有映射
        assert_eq!(result.tool_name_map.len(), 1);

        // 映射中的值应该是原始名称
        let (short, original) = result.tool_name_map.iter().next().unwrap();
        assert_eq!(original, long_tool_name);
        assert!(short.len() <= TOOL_NAME_MAX_LEN);

        // Kiro 请求中的工具名应该是短名称
        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;
        assert_eq!(tools[0].tool_specification.name, *short);
    }

    #[test]
    fn test_tool_name_mapping_in_history() {
        use super::super::types::{Message as AnthropicMessage, Tool as AnthropicTool};

        let long_tool_name =
            "mcp__plugin_very_long_server_name__extremely_long_tool_name_exceeds_63";

        let mut schema = std::collections::HashMap::new();
        schema.insert("type".to_string(), serde_json::json!("object"));
        schema.insert("properties".to_string(), serde_json::json!({}));

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("use the tool"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "text", "text": "calling tool"},
                        {"type": "tool_use", "id": "toolu_01", "name": long_tool_name, "input": {}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "toolu_01", "content": "done"}
                    ]),
                },
            ],
            system: None,
            stream: false,
            tools: Some(vec![AnthropicTool {
                name: long_tool_name.to_string(),
                description: "A test tool".to_string(),
                input_schema: schema,
                tool_type: None,
                max_uses: None,
                cache_control: None,
            }]),
            thinking: None,
            tool_choice: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let result = convert_request(&req).unwrap();
        let short_name = result.tool_name_map.iter().next().unwrap().0.clone();

        // 历史中 assistant 消息的 tool_use name 也应该被映射
        let history = &result.conversation_state.history;
        let mut found = false;
        for msg in history {
            if let Message::Assistant(a) = msg {
                if let Some(ref tool_uses) = a.assistant_response_message.tool_uses {
                    for tu in tool_uses {
                        if tu.tool_use_id == "toolu_01" {
                            assert_eq!(tu.name, short_name, "历史中的 tool_use name 应该是短名称");
                            found = true;
                        }
                    }
                }
            }
        }
        assert!(found, "应该在历史中找到 tool_use");
    }

    #[test]
    fn test_history_tools_added_to_tools_list() {
        use super::super::types::Message as AnthropicMessage;

        // 创建一个请求，历史中有工具使用，但 tools 列表为空
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("Read the file"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "text", "text": "I'll read the file."},
                        {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/test.txt"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "tool-1", "content": "file content"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None, // 没有提供工具定义
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let result = convert_request(&req).unwrap();

        // 验证 tools 列表中包含了历史中使用的工具的占位符定义
        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;

        assert!(!tools.is_empty(), "tools 列表不应为空");
        assert!(
            tools.iter().any(|t| t.tool_specification.name == "read"),
            "tools 列表应包含 'read' 工具的占位符定义"
        );
    }

    #[test]
    fn test_extract_session_id_valid() {
        // 测试有效的 user_id 格式
        let user_id = "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd_account__session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552";
        let session_id = extract_session_id(user_id);
        assert_eq!(
            session_id,
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
    }

    #[test]
    fn test_extract_session_id_json_format() {
        // 测试 JSON 格式的 user_id
        let user_id = r#"{"device_id":"0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd","account_uuid":"","session_id":"8bb5523b-ec7c-4540-a9ca-beb6d79f1552"}"#;
        let session_id = extract_session_id(user_id);
        assert_eq!(
            session_id,
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
    }

    #[test]
    fn test_extract_session_id_json_invalid_session() {
        // 测试 JSON 格式但 session_id 不是有效 UUID
        let user_id = r#"{"device_id":"abc","session_id":"not-a-uuid"}"#;
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_extract_session_id_no_session() {
        // 测试没有 session 的 user_id
        let user_id = "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd";
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_extract_session_id_invalid_uuid() {
        // 测试无效的 UUID 格式
        let user_id = "user_xxx_session_invalid-uuid";
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_convert_request_with_session_metadata() {
        use super::super::types::{Message as AnthropicMessage, Metadata};

        // 测试带有 metadata 的请求，应该使用 session UUID 作为 conversationId
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: Some(Metadata {
                user_id: Some(
                    "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd_account__session_a0662283-7fd3-4399-a7eb-52b9a717ae88".to_string(),
                ),
            }),
        };

        let result = convert_request(&req).unwrap();
        assert_eq!(
            result.conversation_state.conversation_id,
            "a0662283-7fd3-4399-a7eb-52b9a717ae88"
        );
        assert_eq!(
            result.session_affinity_key,
            "a0662283-7fd3-4399-a7eb-52b9a717ae88"
        );
        assert_eq!(
            result.conversation_state.agent_continuation_id.as_deref(),
            Some("a0662283-7fd3-4399-a7eb-52b9a717ae88")
        );
    }

    #[test]
    fn test_convert_request_without_metadata() {
        use super::super::types::Message as AnthropicMessage;

        // 测试没有 metadata 的请求，应该生成新的 UUID
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let result = convert_request(&req).unwrap();
        // 验证生成的是有效的 UUID 格式
        assert_eq!(result.conversation_state.conversation_id.len(), 36);
        assert_eq!(
            result
                .conversation_state
                .conversation_id
                .chars()
                .filter(|c| *c == '-')
                .count(),
            4
        );
        assert_eq!(
            result.conversation_state.agent_continuation_id.as_deref(),
            Some(result.conversation_state.conversation_id.as_str())
        );
    }

    #[test]
    fn test_validate_tool_pairing_orphaned_result() {
        // 测试孤立的 tool_result 被过滤
        // 历史中没有 tool_use，但 tool_results 中有 tool_result
        let history = vec![
            Message::User(HistoryUserMessage::new("Hello", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage::new("Hi there!")),
        ];

        let tool_results = vec![ToolResult::success("orphan-123", "some result")];

        let (filtered, _) = validate_tool_pairing(&history, &tool_results);

        // 孤立的 tool_result 应该被过滤掉
        assert!(filtered.is_empty(), "孤立的 tool_result 应该被过滤");
    }

    #[test]
    fn test_validate_tool_pairing_orphaned_use() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试孤立的 tool_use（有 tool_use 但没有对应的 tool_result）
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-orphan", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        // 没有 tool_result
        let tool_results: Vec<ToolResult> = vec![];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 结果应该为空（因为没有 tool_result）
        // 同时应该返回孤立的 tool_use_id
        assert!(filtered.is_empty());
        assert!(orphaned.contains("tool-orphan"));
    }

    #[test]
    fn test_validate_tool_pairing_valid() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试正常配对的情况
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let tool_results = vec![ToolResult::success("tool-1", "file content")];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 配对成功，应该保留，无孤立
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tool_use_id, "tool-1");
        assert!(orphaned.is_empty());
    }

    #[test]
    fn test_validate_tool_pairing_mixed() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试混合情况：部分配对成功，部分孤立
        let mut assistant_msg = AssistantMessage::new("I'll use two tools.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-2", "write").with_input(serde_json::json!({})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        // tool_results: tool-1 配对，tool-3 孤立
        let tool_results = vec![
            ToolResult::success("tool-1", "result 1"),
            ToolResult::success("tool-3", "orphan result"), // 孤立
        ];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 只有 tool-1 应该保留
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tool_use_id, "tool-1");
        // tool-2 是孤立的 tool_use（无 result），tool-3 是孤立的 tool_result
        assert!(orphaned.contains("tool-2"));
    }

    #[test]
    fn test_validate_tool_pairing_history_already_paired() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试历史中已配对的 tool_use 不应该被报告为孤立
        // 场景：多轮对话中，之前的 tool_use 已经在历史中有对应的 tool_result
        let mut assistant_msg1 = AssistantMessage::new("I'll read the file.");
        assistant_msg1 = assistant_msg1.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        // 构建历史中的 user 消息，包含 tool_result
        let mut user_msg_with_result = UserMessage::new("", "claude-sonnet-4.5");
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(vec![ToolResult::success("tool-1", "file content")]);
        user_msg_with_result = user_msg_with_result.with_context(ctx);

        let history = vec![
            // 第一轮：用户请求
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            // 第一轮：assistant 使用工具
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg1,
            }),
            // 第二轮：用户返回工具结果（历史中已配对）
            Message::User(HistoryUserMessage {
                user_input_message: user_msg_with_result,
            }),
            // 第二轮：assistant 响应
            Message::Assistant(HistoryAssistantMessage::new("The file contains...")),
        ];

        // 当前消息没有 tool_results（用户只是继续对话）
        let tool_results: Vec<ToolResult> = vec![];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 结果应该为空，且不应该有孤立 tool_use
        // 因为 tool-1 已经在历史中配对了
        assert!(filtered.is_empty());
        assert!(orphaned.is_empty());
    }

    #[test]
    fn test_validate_tool_pairing_duplicate_result() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试重复的 tool_result（历史中已配对，当前消息又发送了相同的 tool_result）
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        // 历史中已有 tool_result
        let mut user_msg_with_result = UserMessage::new("", "claude-sonnet-4.5");
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(vec![ToolResult::success("tool-1", "file content")]);
        user_msg_with_result = user_msg_with_result.with_context(ctx);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
            Message::User(HistoryUserMessage {
                user_input_message: user_msg_with_result,
            }),
            Message::Assistant(HistoryAssistantMessage::new("Done")),
        ];

        // 当前消息又发送了相同的 tool_result（重复）
        let tool_results = vec![ToolResult::success("tool-1", "file content again")];

        let (filtered, _) = validate_tool_pairing(&history, &tool_results);

        // 重复的 tool_result 应该被过滤掉
        assert!(filtered.is_empty(), "重复的 tool_result 应该被过滤");
    }

    #[test]
    fn test_convert_assistant_message_tool_use_only() {
        use super::super::types::Message as AnthropicMessage;

        // 测试仅包含 tool_use 的 assistant 消息（无 text 块）
        // Kiro API 要求 content 字段不能为空
        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/test.txt"}}
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");

        // 验证 content 不为空（使用占位符）
        assert!(
            !result.assistant_response_message.content.is_empty(),
            "content 不应为空"
        );
        assert_eq!(
            result.assistant_response_message.content, " ",
            "仅 tool_use 时应使用 ' ' 占位符"
        );

        // 验证 tool_uses 被正确保留
        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应该有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
        assert_eq!(tool_uses[0].name, "read_file");
    }

    #[test]
    fn test_convert_assistant_message_with_text_and_tool_use() {
        use super::super::types::Message as AnthropicMessage;

        // 测试同时包含 text 和 tool_use 的 assistant 消息
        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "Let me read that file for you."},
                {"type": "tool_use", "id": "toolu_02XYZ", "name": "read_file", "input": {"path": "/data.json"}}
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");

        // 验证 content 使用原始文本（不是占位符）
        assert_eq!(
            result.assistant_response_message.content,
            "Let me read that file for you."
        );

        // 验证 tool_uses 被正确保留
        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应该有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_02XYZ");
    }

    #[test]
    fn test_remove_orphaned_tool_uses() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试从历史中移除孤立的 tool_use
        let mut assistant_msg = AssistantMessage::new("I'll use multiple tools.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-2", "write").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-3", "delete").with_input(serde_json::json!({})),
        ]);

        let mut history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        // 移除 tool-1 和 tool-3
        let mut orphaned = std::collections::HashSet::new();
        orphaned.insert("tool-1".to_string());
        orphaned.insert("tool-3".to_string());

        remove_orphaned_tool_uses(&mut history, &orphaned);

        // 验证只剩下 tool-2
        if let Message::Assistant(ref assistant_msg) = history[1] {
            let tool_uses = assistant_msg
                .assistant_response_message
                .tool_uses
                .as_ref()
                .expect("应该还有 tool_uses");
            assert_eq!(tool_uses.len(), 1);
            assert_eq!(tool_uses[0].tool_use_id, "tool-2");
        } else {
            panic!("应该是 Assistant 消息");
        }
    }

    #[test]
    fn test_remove_orphaned_tool_uses_all_removed() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试移除所有 tool_use 后，tool_uses 变为 None
        let mut assistant_msg = AssistantMessage::new("I'll use a tool.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
        ]);

        let mut history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let mut orphaned = std::collections::HashSet::new();
        orphaned.insert("tool-1".to_string());

        remove_orphaned_tool_uses(&mut history, &orphaned);

        // 验证 tool_uses 变为 None
        if let Message::Assistant(ref assistant_msg) = history[1] {
            assert!(
                assistant_msg.assistant_response_message.tool_uses.is_none(),
                "移除所有 tool_use 后应为 None"
            );
        } else {
            panic!("应该是 Assistant 消息");
        }
    }

    #[test]
    fn test_merge_consecutive_assistant_messages() {
        // 测试连续 assistant 消息被正确合并（Issue #79）
        use super::super::types::Message as AnthropicMessage;

        let msg1 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "Let me think about this..."},
                {"type": "text", "text": " "}
            ]),
        };

        let msg2 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "I should read the file."},
                {"type": "text", "text": "Let me read that file."},
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/test.txt"}}
            ]),
        };

        let messages: Vec<&AnthropicMessage> = vec![&msg1, &msg2];
        let result = merge_assistant_messages(&messages, &mut HashMap::new()).expect("合并应成功");

        let content = &result.assistant_response_message.content;
        assert!(content.contains("<thinking>"), "应包含 thinking 标签");
        assert!(
            content.contains("Let me read that file"),
            "应包含第二条消息的 text 内容"
        );

        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
    }

    #[test]
    fn test_consecutive_assistant_with_tool_use_result_pairing() {
        // 测试 Issue #79 的完整场景
        use super::super::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("Read the config file"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "thinking", "thinking": "I need to read the file..."},
                        {"type": "text", "text": " "}
                    ]),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "thinking", "thinking": "Let me read the config."},
                        {"type": "text", "text": "I'll read the config file for you."},
                        {"type": "tool_use", "id": "toolu_01XYZ", "name": "read_file", "input": {"path": "/config.json"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "toolu_01XYZ", "content": "{\"key\": \"value\"}"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            response_format: None,
            metadata: None,
        };

        let result = convert_request(&req);
        assert!(
            result.is_ok(),
            "连续 assistant 消息场景不应报错: {:?}",
            result.err()
        );

        let state = result.unwrap().conversation_state;
        let mut found_tool_use = false;
        for msg in &state.history {
            if let Message::Assistant(assistant_msg) = msg {
                if let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                    if tool_uses.iter().any(|t| t.tool_use_id == "toolu_01XYZ") {
                        found_tool_use = true;
                        break;
                    }
                }
            }
        }
        assert!(found_tool_use, "合并后的 assistant 消息应包含 tool_use");
    }
}
