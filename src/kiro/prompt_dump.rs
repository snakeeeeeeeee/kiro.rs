use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::kiro::settings::RuntimeSettings;

#[derive(Debug, Clone)]
pub struct PromptDump {
    inner: Arc<PromptDumpInner>,
}

#[derive(Debug)]
struct PromptDumpInner {
    request_id: String,
    created_at: DateTime<Utc>,
    model: String,
    model_dir: PathBuf,
    dir: PathBuf,
    max_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptDumpMetaUpdate {
    pub route: String,
    pub model: String,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_classification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_text_only: Option<bool>,
    #[serde(default)]
    pub truncated: bool,
}

impl PromptDump {
    pub fn maybe_create(
        settings: &RuntimeSettings,
        route: &str,
        model: &str,
        stream: bool,
        client_request: &impl Serialize,
    ) -> Option<Self> {
        if !settings.prompt_dump_enabled
            || !prompt_dump_model_allowed(&settings.prompt_dump_models, model)
        {
            return None;
        }

        let request_id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let safe_model = sanitize_path_segment(model);
        let model_dir = Path::new(&settings.prompt_dump_dir).join(safe_model);
        let dir = model_dir.join(&request_id);
        if let Err(err) = fs::create_dir_all(&dir) {
            tracing::warn!(error = %err, path = %dir.display(), "prompt_dump_create_dir_failed");
            return None;
        }

        let dump = Self {
            inner: Arc::new(PromptDumpInner {
                request_id,
                created_at,
                model: model.to_string(),
                model_dir,
                dir,
                max_bytes: settings.prompt_dump_max_bytes,
            }),
        };
        dump.write_json("client_request.json", client_request);
        dump.write_json(
            "meta.json",
            &json!({
                "request_id": dump.request_id(),
                "created_at": dump.inner.created_at.to_rfc3339(),
                "route": route,
                "model": model,
                "stream": stream,
                "truncated": false
            }),
        );
        dump.update_latest(route, stream);
        Some(dump)
    }

    pub fn request_id(&self) -> &str {
        &self.inner.request_id
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    #[cfg(test)]
    pub fn model_dir(&self) -> &Path {
        &self.inner.model_dir
    }

    pub fn write_json(&self, filename: &str, value: &impl Serialize) {
        match serde_json::to_vec_pretty(value) {
            Ok(bytes) => self.write_limited(filename, &bytes),
            Err(err) => tracing::warn!(
                error = %err,
                filename,
                request_id = self.request_id(),
                "prompt_dump_json_serialize_failed"
            ),
        }
    }

    pub fn write_text(&self, filename: &str, text: &str) {
        self.write_limited(filename, text.as_bytes());
    }

    pub fn append_text(&self, filename: &str, text: &str) {
        self.append_limited(filename, text.as_bytes());
    }

    pub fn append_json_line(&self, filename: &str, value: &impl Serialize) {
        match serde_json::to_string(value) {
            Ok(mut line) => {
                line.push('\n');
                self.append_text(filename, &line);
            }
            Err(err) => tracing::warn!(
                error = %err,
                filename,
                request_id = self.request_id(),
                "prompt_dump_jsonl_serialize_failed"
            ),
        }
    }

    pub fn update_meta(&self, update: PromptDumpMetaUpdate) {
        let path = self.inner.dir.join("meta.json");
        let mut meta = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| json!({ "request_id": self.request_id() }));

        if let Value::Object(map) = &mut meta {
            map.insert("updated_at".to_string(), json!(Utc::now().to_rfc3339()));
            map.insert("route".to_string(), json!(update.route));
            map.insert("model".to_string(), json!(update.model));
            map.insert("stream".to_string(), json!(update.stream));
            if let Some(credential_id) = update.credential_id {
                map.insert("credential_id".to_string(), json!(credential_id));
            }
            if let Some(attempts) = update.attempts {
                map.insert("attempts".to_string(), json!(attempts));
            }
            if let Some(status) = update.status {
                map.insert("status".to_string(), json!(status));
            }
            if let Some(duration_ms) = update.duration_ms {
                map.insert("duration_ms".to_string(), json!(duration_ms));
            }
            if let Some(classification) = update.signature_classification {
                map.insert(
                    "signature_classification".to_string(),
                    json!(classification),
                );
            }
            if let Some(request_kind) = update.request_kind {
                map.insert("request_kind".to_string(), json!(request_kind));
            }
            if let Some(expected_text_only) = update.expected_text_only {
                map.insert("expected_text_only".to_string(), json!(expected_text_only));
            }
            let existing_truncated = map
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            map.insert(
                "truncated".to_string(),
                json!(existing_truncated || update.truncated),
            );
        }
        self.write_json("meta.json", &meta);
    }

    fn write_limited(&self, filename: &str, bytes: &[u8]) {
        let truncated = bytes.len() > self.inner.max_bytes;
        let limit = bytes.len().min(self.inner.max_bytes);
        let path = self.inner.dir.join(filename);
        if let Err(err) = fs::write(&path, &bytes[..limit]) {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                request_id = self.request_id(),
                "prompt_dump_write_failed"
            );
        }
        if truncated {
            self.mark_truncated(filename);
        }
    }

    fn append_limited(&self, filename: &str, bytes: &[u8]) {
        let path = self.inner.dir.join(filename);
        let current_len = fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        if current_len >= self.inner.max_bytes {
            self.mark_truncated(filename);
            return;
        }
        let remaining = self.inner.max_bytes - current_len;
        let truncated = bytes.len() > remaining;
        let limit = bytes.len().min(remaining);
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(&bytes[..limit]) {
                    tracing::warn!(
                        error = %err,
                        path = %path.display(),
                        request_id = self.request_id(),
                        "prompt_dump_append_failed"
                    );
                }
            }
            Err(err) => tracing::warn!(
                error = %err,
                path = %path.display(),
                request_id = self.request_id(),
                "prompt_dump_open_failed"
            ),
        }
        if truncated {
            self.mark_truncated(filename);
        }
    }

    fn mark_truncated(&self, filename: &str) {
        let path = self.inner.dir.join("meta.json");
        let mut meta = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| json!({ "request_id": self.request_id() }));
        if let Value::Object(map) = &mut meta {
            map.insert("truncated".to_string(), json!(true));
            let files = map
                .entry("truncated_files")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(items) = files {
                if !items.iter().any(|item| item.as_str() == Some(filename)) {
                    items.push(json!(filename));
                }
            }
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&meta) {
            let _ = fs::write(path, bytes);
        }
    }

    fn update_latest(&self, route: &str, stream: bool) {
        let latest_path = self.inner.model_dir.join("latest.json");
        let latest = json!({
            "request_id": self.request_id(),
            "created_at": self.inner.created_at.to_rfc3339(),
            "route": route,
            "model": self.inner.model,
            "stream": stream,
            "dir": self.inner.dir.to_string_lossy()
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&latest) {
            if let Err(err) = fs::write(&latest_path, bytes) {
                tracing::warn!(
                    error = %err,
                    path = %latest_path.display(),
                    request_id = self.request_id(),
                    "prompt_dump_latest_write_failed"
                );
            }
        }
    }
}

pub fn prompt_dump_model_allowed(models: &str, model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    models
        .split(',')
        .map(|item| item.trim().to_ascii_lowercase())
        .any(|item| item == "*" || item == model)
}

fn sanitize_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::Config;

    #[test]
    fn model_allowlist_matches_case_insensitively() {
        assert!(prompt_dump_model_allowed(
            "claude-opus-4-6,claude-opus-4-7",
            "Claude-Opus-4-7"
        ));
        assert!(!prompt_dump_model_allowed(
            "claude-opus-4-6",
            "claude-sonnet-4-6"
        ));
        assert!(prompt_dump_model_allowed("*", "anything"));
    }

    #[test]
    fn disabled_dump_does_not_create_context() {
        let settings = RuntimeSettings::from_config(&Config::default());
        let dump = PromptDump::maybe_create(
            &settings,
            "/v1/messages",
            "claude-opus-4-7",
            true,
            &json!({"ok": true}),
        );
        assert!(dump.is_none());
    }

    #[test]
    fn dump_uses_model_bucket_and_latest_pointer() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        let dir = std::env::temp_dir().join(format!("kiro-prompt-dump-test-{}", Uuid::new_v4()));
        settings.prompt_dump_enabled = true;
        settings.prompt_dump_dir = dir.to_string_lossy().to_string();
        settings.prompt_dump_models = "*".to_string();
        let dump = PromptDump::maybe_create(
            &settings,
            "/v1/messages",
            "claude/op us:4.7",
            true,
            &json!({"ok": true}),
        )
        .unwrap();

        assert_eq!(dump.model_dir(), dir.join("claude_op_us_4_7"));
        assert!(dump.dir().starts_with(dump.model_dir()));
        assert!(dump.dir().join("client_request.json").exists());

        let latest = fs::read_to_string(dump.model_dir().join("latest.json")).unwrap();
        assert!(latest.contains(dump.request_id()));
        assert!(latest.contains("\"model\": \"claude/op us:4.7\""));
        assert!(
            !dump
                .dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("2026")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dump_truncates_large_file_and_marks_meta() {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        let dir = std::env::temp_dir().join(format!("kiro-prompt-dump-test-{}", Uuid::new_v4()));
        settings.prompt_dump_enabled = true;
        settings.prompt_dump_dir = dir.to_string_lossy().to_string();
        settings.prompt_dump_max_bytes = 10_000;
        let dump = PromptDump::maybe_create(
            &settings,
            "/v1/messages",
            "claude-opus-4-7",
            true,
            &json!({"ok": true}),
        )
        .unwrap();
        dump.write_text("upstream_response.raw", &"x".repeat(20_000));
        let meta = fs::read_to_string(dump.dir().join("meta.json")).unwrap();
        assert!(meta.contains("\"truncated\": true"));
        assert_eq!(
            fs::metadata(dump.dir().join("upstream_response.raw"))
                .unwrap()
                .len(),
            10_000
        );
        let _ = fs::remove_dir_all(dir);
    }
}
