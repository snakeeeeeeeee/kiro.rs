use serde_json::Value;

use crate::kiro::settings::RuntimeSettings;

const TRUNCATION_MARKER: &str = "\n[truncated by message pruning]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePruningConfig {
    pub enabled: bool,
    pub max_request_bytes: usize,
    pub keep_recent_messages: usize,
    pub max_history_entry_bytes: usize,
    pub max_truncated_content_bytes: usize,
}

impl From<&RuntimeSettings> for MessagePruningConfig {
    fn from(settings: &RuntimeSettings) -> Self {
        Self {
            enabled: settings.message_pruning_enabled,
            max_request_bytes: settings.message_pruning_max_request_bytes,
            keep_recent_messages: settings.message_pruning_keep_recent_messages,
            max_history_entry_bytes: settings.message_pruning_max_history_entry_bytes,
            max_truncated_content_bytes: settings.message_pruning_max_truncated_content_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePruningOutcome {
    Noop,
    Skipped(MessagePruningStats),
    Pruned {
        body: String,
        stats: MessagePruningStats,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessagePruningStats {
    pub original_bytes: usize,
    pub final_bytes: usize,
    pub original_history_len: usize,
    pub final_history_len: usize,
    pub removed_entries: usize,
    pub truncated_entries: usize,
    pub orphaned_tool_results_removed: usize,
    pub empty_tool_uses_stripped: usize,
    pub aligned_leading_entries_removed: usize,
    pub under_limit: bool,
}

impl MessagePruningStats {
    fn new(original_bytes: usize, original_history_len: usize) -> Self {
        Self {
            original_bytes,
            final_bytes: original_bytes,
            original_history_len,
            final_history_len: original_history_len,
            under_limit: false,
            ..Default::default()
        }
    }
}

pub fn guard_kiro_payload(body: &str, config: &MessagePruningConfig) -> MessagePruningOutcome {
    let original_bytes = body.len();
    if original_bytes <= config.max_request_bytes {
        return MessagePruningOutcome::Noop;
    }

    let original_history_len = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/conversationState/history")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or_default();
    let mut skipped = MessagePruningStats::new(original_bytes, original_history_len);
    skipped.under_limit = false;

    if !config.enabled {
        return MessagePruningOutcome::Skipped(skipped);
    }

    let Ok(mut payload) = serde_json::from_str::<Value>(body) else {
        return MessagePruningOutcome::Skipped(skipped);
    };

    let Some(history) = payload
        .pointer("/conversationState/history")
        .and_then(Value::as_array)
    else {
        return MessagePruningOutcome::Skipped(skipped);
    };

    if history.is_empty() {
        return MessagePruningOutcome::Skipped(skipped);
    }

    let mut stats = MessagePruningStats::new(original_bytes, history.len());

    strip_empty_tool_uses(&mut payload, &mut stats);
    trim_old_history(&mut payload, config, &mut stats);
    align_history_to_user_message(&mut payload, config, &mut stats);
    repair_orphaned_tool_results(&mut payload, &mut stats);

    if serialized_len(&payload) > config.max_request_bytes {
        truncate_large_history_entries(&mut payload, config, &mut stats);
        repair_orphaned_tool_results(&mut payload, &mut stats);
    }

    stats.final_bytes = serialized_len(&payload);
    stats.final_history_len = history_len(&payload);
    stats.under_limit = stats.final_bytes <= config.max_request_bytes;

    let changed = stats.removed_entries > 0
        || stats.truncated_entries > 0
        || stats.orphaned_tool_results_removed > 0
        || stats.empty_tool_uses_stripped > 0
        || stats.aligned_leading_entries_removed > 0;
    if !changed {
        return MessagePruningOutcome::Skipped(stats);
    }

    match serde_json::to_string(&payload) {
        Ok(body) => MessagePruningOutcome::Pruned { body, stats },
        Err(_) => MessagePruningOutcome::Skipped(stats),
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

fn history_len(payload: &Value) -> usize {
    payload
        .pointer("/conversationState/history")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

fn history_mut(payload: &mut Value) -> Option<&mut Vec<Value>> {
    payload
        .pointer_mut("/conversationState/history")
        .and_then(Value::as_array_mut)
}

fn strip_empty_tool_uses(payload: &mut Value, stats: &mut MessagePruningStats) {
    let Some(history) = history_mut(payload) else {
        return;
    };

    for entry in history {
        let Some(assistant) = entry
            .get_mut("assistantResponseMessage")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let empty = assistant
            .get("toolUses")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        if empty {
            assistant.remove("toolUses");
            stats.empty_tool_uses_stripped += 1;
        }
    }
}

fn trim_old_history(
    payload: &mut Value,
    config: &MessagePruningConfig,
    stats: &mut MessagePruningStats,
) {
    loop {
        if serialized_len(payload) <= config.max_request_bytes {
            break;
        }

        let remove_count = {
            let Some(history) = history_mut(payload) else {
                break;
            };
            if history.len() <= config.keep_recent_messages {
                break;
            }

            let removable = history.len().saturating_sub(config.keep_recent_messages);
            if removable >= 2 && history.len() >= 2 {
                2
            } else {
                1
            }
        };

        let Some(history) = history_mut(payload) else {
            break;
        };
        for _ in 0..remove_count {
            if history.len() <= config.keep_recent_messages {
                break;
            }
            history.remove(0);
            stats.removed_entries += 1;
        }
    }
}

fn align_history_to_user_message(
    payload: &mut Value,
    config: &MessagePruningConfig,
    stats: &mut MessagePruningStats,
) {
    let Some(history) = history_mut(payload) else {
        return;
    };
    while history.len() > config.keep_recent_messages
        && history
            .first()
            .is_some_and(|entry| entry.get("userInputMessage").is_none())
    {
        history.remove(0);
        stats.removed_entries += 1;
        stats.aligned_leading_entries_removed += 1;
    }
}

fn repair_orphaned_tool_results(payload: &mut Value, stats: &mut MessagePruningStats) {
    let Some(history) = history_mut(payload) else {
        return;
    };

    for index in 0..history.len() {
        let valid_ids = if index > 0 {
            assistant_tool_use_ids(&history[index - 1])
        } else {
            Vec::new()
        };
        let Some(user_message) = history[index]
            .get_mut("userInputMessage")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };

        let Some(context) = user_message
            .get_mut("userInputMessageContext")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let Some(tool_results) = context.get_mut("toolResults").and_then(Value::as_array_mut)
        else {
            continue;
        };

        let original_len = tool_results.len();
        if original_len == 0 {
            continue;
        }

        let mut orphaned_text = Vec::new();
        tool_results.retain(|result| {
            let tool_use_id = result.get("toolUseId").and_then(Value::as_str);
            let keep = tool_use_id.is_some_and(|id| valid_ids.iter().any(|valid| valid == id));
            if !keep {
                collect_tool_result_text(result, &mut orphaned_text);
            }
            keep
        });

        let removed = original_len.saturating_sub(tool_results.len());
        if removed == 0 {
            continue;
        }
        stats.orphaned_tool_results_removed += removed;

        if tool_results.is_empty() {
            context.remove("toolResults");
        }
        if context.is_empty() {
            user_message.remove("userInputMessageContext");
        }
        append_orphaned_text(user_message, &orphaned_text);
    }
}

fn assistant_tool_use_ids(entry: &Value) -> Vec<String> {
    entry
        .get("assistantResponseMessage")
        .and_then(|assistant| assistant.get("toolUses"))
        .and_then(Value::as_array)
        .map(|tool_uses| {
            tool_uses
                .iter()
                .filter_map(|tool_use| {
                    tool_use
                        .get("toolUseId")
                        .or_else(|| tool_use.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn collect_tool_result_text(result: &Value, out: &mut Vec<String>) {
    if let Some(text) = result.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            out.push(text.to_string());
        }
        return;
    }

    if let Some(parts) = result.get("content").and_then(Value::as_array) {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    out.push(text.to_string());
                }
            }
        }
    }
}

fn append_orphaned_text(user_message: &mut serde_json::Map<String, Value>, text_parts: &[String]) {
    if text_parts.is_empty() {
        return;
    }

    let mut append = String::from("\n[trimmed tool result] ");
    append.push_str(&text_parts.join("\n"));
    match user_message.get_mut("content") {
        Some(Value::String(content)) => content.push_str(&append),
        _ => {
            user_message.insert("content".to_string(), Value::String(append));
        }
    }
}

fn truncate_large_history_entries(
    payload: &mut Value,
    config: &MessagePruningConfig,
    stats: &mut MessagePruningStats,
) {
    let Some(history) = history_mut(payload) else {
        return;
    };

    for entry in history {
        if serialized_len(entry) <= config.max_history_entry_bytes {
            continue;
        }

        let before = serialized_len(entry);
        truncate_entry_text(entry, config.max_truncated_content_bytes);
        if serialized_len(entry) < before {
            stats.truncated_entries += 1;
        }
    }
}

fn truncate_entry_text(entry: &mut Value, max_bytes: usize) {
    if let Some(content) = entry
        .pointer_mut("/userInputMessage/content")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    {
        if content.len() > max_bytes {
            let truncated = truncate_utf8_with_marker(&content, max_bytes);
            if let Some(slot) = entry.pointer_mut("/userInputMessage/content") {
                *slot = Value::String(truncated);
            }
        }
    }

    if let Some(content) = entry
        .pointer_mut("/assistantResponseMessage/content")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    {
        if content.len() > max_bytes {
            let truncated = truncate_utf8_with_marker(&content, max_bytes);
            if let Some(slot) = entry.pointer_mut("/assistantResponseMessage/content") {
                *slot = Value::String(truncated);
            }
        }
    }

    if let Some(tool_results) = entry
        .pointer_mut("/userInputMessage/userInputMessageContext/toolResults")
        .and_then(Value::as_array_mut)
    {
        for result in tool_results {
            truncate_tool_result_content(result, max_bytes);
        }
    }
}

fn truncate_tool_result_content(result: &mut Value, max_bytes: usize) {
    if let Some(content) = result.get("content").and_then(Value::as_str) {
        if content.len() > max_bytes {
            result["content"] = Value::String(truncate_utf8_with_marker(content, max_bytes));
        }
        return;
    }

    let Some(parts) = result.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    for part in parts {
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.len() > max_bytes {
            part["text"] = Value::String(truncate_utf8_with_marker(text, max_bytes));
        }
    }
}

fn truncate_utf8_with_marker(value: &str, max_bytes: usize) -> String {
    let marker_bytes = TRUNCATION_MARKER.len();
    let keep_bytes = max_bytes.saturating_sub(marker_bytes).max(1);
    let mut end = keep_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = value[..end].to_string();
    result.push_str(TRUNCATION_MARKER);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn enabled_config(max_request_bytes: usize) -> MessagePruningConfig {
        MessagePruningConfig {
            enabled: true,
            max_request_bytes,
            keep_recent_messages: 2,
            max_history_entry_bytes: 300,
            max_truncated_content_bytes: 80,
        }
    }

    fn payload_with_history(history: Vec<Value>, current: &str) -> Value {
        json!({
            "conversationState": {
                "history": history,
                "currentMessage": {
                    "userInputMessage": {
                        "content": current,
                        "modelId": "claude-opus-4.8"
                    }
                }
            }
        })
    }

    #[test]
    fn disabled_over_limit_is_skipped_without_modifying_body() {
        let payload = payload_with_history(
            vec![
                json!({"userInputMessage": {"content": "old old old"}}),
                json!({"assistantResponseMessage": {"content": "answer answer answer"}}),
            ],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let config = MessagePruningConfig {
            enabled: false,
            max_request_bytes: 10,
            keep_recent_messages: 2,
            max_history_entry_bytes: 300,
            max_truncated_content_bytes: 80,
        };

        match guard_kiro_payload(&body, &config) {
            MessagePruningOutcome::Skipped(stats) => {
                assert_eq!(stats.original_bytes, body.len());
                assert_eq!(stats.final_bytes, body.len());
            }
            other => panic!("expected skipped, got {other:?}"),
        }
    }

    #[test]
    fn under_limit_is_noop() {
        let payload = payload_with_history(
            vec![json!({"userInputMessage": {"content": "old"}})],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let config = enabled_config(body.len() + 1);
        assert_eq!(
            guard_kiro_payload(&body, &config),
            MessagePruningOutcome::Noop
        );
    }

    #[test]
    fn trims_oldest_history_until_under_limit_and_keeps_current_message() {
        let big = "x".repeat(120);
        let payload = payload_with_history(
            vec![
                json!({"userInputMessage": {"content": big}}),
                json!({"assistantResponseMessage": {"content": big}}),
                json!({"userInputMessage": {"content": "recent user"}}),
                json!({"assistantResponseMessage": {"content": "recent assistant"}}),
            ],
            "do not remove current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let outcome = guard_kiro_payload(&body, &enabled_config(330));
        let MessagePruningOutcome::Pruned { body, stats } = outcome else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let history = pruned
            .pointer("/conversationState/history")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(history.len() >= 2);
        assert!(body.len() <= 330);
        assert_eq!(
            pruned
                .pointer("/conversationState/currentMessage/userInputMessage/content")
                .and_then(Value::as_str),
            Some("do not remove current")
        );
        assert!(stats.removed_entries >= 2);
        assert!(stats.under_limit);
    }

    #[test]
    fn keeps_configured_recent_history_even_when_still_over_limit() {
        let payload = payload_with_history(
            vec![
                json!({"userInputMessage": {"content": "old user"}}),
                json!({"assistantResponseMessage": {"content": "old assistant"}}),
                json!({"userInputMessage": {"content": "recent user"}}),
                json!({"assistantResponseMessage": {"content": "recent assistant"}}),
            ],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let mut config = enabled_config(1);
        config.keep_recent_messages = 2;
        config.max_history_entry_bytes = 10_000;
        let MessagePruningOutcome::Pruned { body, stats } = guard_kiro_payload(&body, &config)
        else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let history = pruned
            .pointer("/conversationState/history")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(stats.final_history_len, 2);
        assert!(!stats.under_limit);
    }

    #[test]
    fn strips_empty_tool_uses_and_aligns_history_to_user() {
        let payload = payload_with_history(
            vec![
                json!({"assistantResponseMessage": {"content": "leading", "toolUses": []}}),
                json!({"userInputMessage": {"content": "kept user"}}),
                json!({"assistantResponseMessage": {"content": "kept assistant", "toolUses": []}}),
            ],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let mut config = enabled_config(body.len() - 1);
        config.keep_recent_messages = 1;
        let MessagePruningOutcome::Pruned { body, stats } = guard_kiro_payload(&body, &config)
        else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let history = pruned
            .pointer("/conversationState/history")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(history.first().unwrap().get("userInputMessage").is_some());
        assert!(history.iter().all(|entry| {
            entry
                .pointer("/assistantResponseMessage/toolUses")
                .and_then(Value::as_array)
                .is_none()
        }));
        assert!(stats.empty_tool_uses_stripped >= 1);
    }

    #[test]
    fn repairs_orphaned_tool_results_and_preserves_matching_results() {
        let payload = payload_with_history(
            vec![
                json!({"userInputMessage": {
                    "content": "orphan",
                    "userInputMessageContext": {
                        "toolResults": [{"toolUseId": "missing", "content": "lost output"}]
                    }
                }}),
                json!({"assistantResponseMessage": {
                    "content": "tool",
                    "toolUses": [{"toolUseId": "tool_1", "name": "read"}]
                }}),
                json!({"userInputMessage": {
                    "content": "result",
                    "userInputMessageContext": {
                        "toolResults": [
                            {"toolUseId": "tool_1", "content": "keep output"},
                            {"toolUseId": "missing_2", "content": "orphan output"}
                        ]
                    }
                }}),
            ],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let mut config = enabled_config(body.len() - 1);
        config.keep_recent_messages = 3;
        let MessagePruningOutcome::Pruned { body, stats } = guard_kiro_payload(&body, &config)
        else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let history = pruned
            .pointer("/conversationState/history")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(
            history[0]
                .pointer("/userInputMessage/userInputMessageContext/toolResults")
                .is_none()
        );
        assert!(
            history[0]
                .pointer("/userInputMessage/content")
                .and_then(Value::as_str)
                .unwrap()
                .contains("lost output")
        );
        let results = history[2]
            .pointer("/userInputMessage/userInputMessageContext/toolResults")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].get("toolUseId").and_then(Value::as_str),
            Some("tool_1")
        );
        assert_eq!(stats.orphaned_tool_results_removed, 2);
    }

    #[test]
    fn backfills_orphaned_array_tool_result_text_to_user_content() {
        let payload = payload_with_history(
            vec![json!({"userInputMessage": {
                "content": "user",
                "userInputMessageContext": {
                    "toolResults": [{
                        "toolUseId": "missing",
                        "content": [
                            {"type": "text", "text": "array output"},
                            {"type": "text", "text": "second part"}
                        ]
                    }]
                }
            }})],
            "current",
        );
        let body = serde_json::to_string(&payload).unwrap();
        let mut config = enabled_config(body.len() - 1);
        config.keep_recent_messages = 1;
        let MessagePruningOutcome::Pruned { body, stats } = guard_kiro_payload(&body, &config)
        else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let content = pruned
            .pointer("/conversationState/history/0/userInputMessage/content")
            .and_then(Value::as_str)
            .unwrap();
        assert!(content.contains("array output"));
        assert!(content.contains("second part"));
        assert!(
            pruned
                .pointer("/conversationState/history/0/userInputMessage/userInputMessageContext")
                .is_none()
        );
        assert_eq!(stats.orphaned_tool_results_removed, 1);
    }

    #[test]
    fn truncates_large_history_entries_without_touching_current_message() {
        let payload = payload_with_history(
            vec![
                json!({"userInputMessage": {"content": "x".repeat(600)}}),
                json!({"assistantResponseMessage": {"content": "recent"}}),
            ],
            &"current ".repeat(100),
        );
        let body = serde_json::to_string(&payload).unwrap();
        let mut config = enabled_config(260);
        config.keep_recent_messages = 2;
        config.max_history_entry_bytes = 100;
        config.max_truncated_content_bytes = 40;
        let MessagePruningOutcome::Pruned { body, stats } = guard_kiro_payload(&body, &config)
        else {
            panic!("expected pruned");
        };
        let pruned: Value = serde_json::from_str(&body).unwrap();
        let current_content = "current ".repeat(100);
        let history_content = pruned
            .pointer("/conversationState/history/0/userInputMessage/content")
            .and_then(Value::as_str)
            .unwrap();
        assert!(history_content.contains("[truncated by message pruning]"));
        assert_eq!(
            pruned
                .pointer("/conversationState/currentMessage/userInputMessage/content")
                .and_then(Value::as_str),
            Some(current_content.as_str())
        );
        assert_eq!(stats.truncated_entries, 1);
    }
}
