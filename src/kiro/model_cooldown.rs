use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCooldownSnapshot {
    pub model: String,
    pub cooldown_until: String,
    pub remaining_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct ModelCooldownEntry {
    cooldown_until: DateTime<Utc>,
    reason: String,
}

#[derive(Default)]
pub struct ModelCooldownManager {
    entries: Mutex<HashMap<String, ModelCooldownEntry>>,
}

impl ModelCooldownManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(&self, model: &str) -> Option<ModelCooldownSnapshot> {
        let now = Utc::now();
        let mut entries = self.entries.lock();
        if entries
            .get(model)
            .map(|entry| entry.cooldown_until <= now)
            .unwrap_or(false)
        {
            entries.remove(model);
            return None;
        }

        entries
            .get(model)
            .map(|entry| snapshot_for(model, entry, now))
    }

    pub fn set_cooldown(&self, model: &str, cooldown_ms: u64, reason: impl Into<String>) {
        if cooldown_ms == 0 {
            return;
        }
        let until = Utc::now() + Duration::milliseconds(cooldown_ms as i64);
        let reason = reason.into();
        self.entries.lock().insert(
            model.to_string(),
            ModelCooldownEntry {
                cooldown_until: until,
                reason: reason.clone(),
            },
        );
        tracing::warn!(
            model = %model,
            reason = %reason,
            cooldown_until = %until.to_rfc3339(),
            cooldown_ms,
            "模型进入冷却"
        );
    }

    pub fn snapshot(&self) -> Vec<ModelCooldownSnapshot> {
        let now = Utc::now();
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| entry.cooldown_until > now);
        let mut result: Vec<_> = entries
            .iter()
            .map(|(model, entry)| snapshot_for(model, entry, now))
            .collect();
        result.sort_by(|a, b| a.model.cmp(&b.model));
        result
    }
}

fn snapshot_for(
    model: &str,
    entry: &ModelCooldownEntry,
    now: DateTime<Utc>,
) -> ModelCooldownSnapshot {
    let remaining_ms = entry
        .cooldown_until
        .signed_duration_since(now)
        .num_milliseconds()
        .max(0) as u64;
    ModelCooldownSnapshot {
        model: model.to_string(),
        cooldown_until: entry.cooldown_until.to_rfc3339(),
        remaining_ms,
        reason: entry.reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_snapshot_and_expiry() {
        let manager = ModelCooldownManager::new();
        manager.set_cooldown("claude-opus-4.7", 25, "INSUFFICIENT_MODEL_CAPACITY");

        let snapshot = manager.check("claude-opus-4.7").expect("cooldown exists");
        assert_eq!(snapshot.model, "claude-opus-4.7");
        assert_eq!(snapshot.reason, "INSUFFICIENT_MODEL_CAPACITY");

        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(manager.check("claude-opus-4.7").is_none());
    }
}
