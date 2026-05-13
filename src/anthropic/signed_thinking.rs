//! Opus 4.7 signed-thinking diagnostics/cache helpers.
//!
//! This cache only stores real upstream signatures observed next to upstream
//! reasoning text. It never creates or repairs signatures.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedThinkingMode {
    Off,
    Diagnose,
    CacheOnly,
    HistoryExperiment,
}

impl SignedThinkingMode {
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "diagnose" => Self::Diagnose,
            "cache_only" | "cache-only" => Self::CacheOnly,
            "history_experiment" | "history-experiment" => Self::HistoryExperiment,
            _ => Self::Off,
        }
    }

    pub fn cache_enabled(self) -> bool {
        matches!(self, Self::CacheOnly | Self::HistoryExperiment)
    }

    pub fn diagnostics_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Diagnose => "diagnose",
            Self::CacheOnly => "cache_only",
            Self::HistoryExperiment => "history_experiment",
        }
    }
}

#[derive(Debug, Clone)]
struct CachedSignature {
    #[cfg_attr(not(test), allow(dead_code))]
    signature: String,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct SignedThinkingCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, CachedSignature>>,
}

impl SignedThinkingCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn store(&self, model: &str, thinking: &str, signature: &str) -> bool {
        if thinking.trim().is_empty() || signature.trim().is_empty() {
            return false;
        }
        let key = cache_key(model, thinking);
        let mut entries = self.entries.lock().expect("signed-thinking cache poisoned");
        prune_expired_locked(&mut entries);
        entries.insert(
            key,
            CachedSignature {
                signature: signature.to_string(),
                expires_at: Instant::now() + self.ttl,
            },
        );
        true
    }

    #[cfg(test)]
    pub fn get(&self, model: &str, thinking: &str) -> Option<String> {
        let key = cache_key(model, thinking);
        let mut entries = self.entries.lock().expect("signed-thinking cache poisoned");
        prune_expired_locked(&mut entries);
        entries.get(&key).map(|entry| entry.signature.clone())
    }
}

fn prune_expired_locked(entries: &mut HashMap<String, CachedSignature>) {
    let now = Instant::now();
    entries.retain(|_, entry| entry.expires_at > now);
}

fn cache_key(model: &str, thinking: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_group(model).as_bytes());
    hasher.update(b"\0");
    hasher.update(thinking.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn model_group(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    lower
        .strip_suffix("-thinking")
        .unwrap_or(lower.as_str())
        .replace('.', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_rejects_empty_signature_or_thinking() {
        let cache = SignedThinkingCache::new(Duration::from_secs(60));
        assert!(!cache.store("claude-opus-4-7", "thinking", ""));
        assert!(!cache.store("claude-opus-4-7", "", "sig"));
        assert!(cache.get("claude-opus-4-7", "thinking").is_none());
    }

    #[test]
    fn cache_normalizes_model_group() {
        let cache = SignedThinkingCache::new(Duration::from_secs(60));
        assert!(cache.store("claude-opus-4.7-thinking", "abc", "sig_real"));
        assert_eq!(
            cache.get("claude-opus-4-7", "abc"),
            Some("sig_real".to_string())
        );
    }
}
