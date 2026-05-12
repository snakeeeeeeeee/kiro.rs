//! 推理内容事件
//!
//! 处理 reasoningContentEvent 类型的事件。Kiro 在 Claude 4 推理输出中会返回
//! 文本增量和签名增量，对应 Anthropic extended thinking 的 thinking/signature。

use serde::Deserialize;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningContentEvent {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl EventPayload for ReasoningContentEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}
