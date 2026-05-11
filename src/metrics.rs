use std::collections::{HashMap, VecDeque};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Serialize;

const WINDOW_SECS: i64 = 300;
const MAX_SAMPLES: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamOutcome {
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct RequestTimingSample {
    pub completed_at: DateTime<Utc>,
    pub model: String,
    pub stream: bool,
    pub credential_id: Option<u64>,
    pub status: Option<u16>,
    pub outcome: UpstreamOutcome,
    pub attempts: usize,
    pub queue_ms: u64,
    pub acquire_ms: u64,
    pub upstream_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetricsSnapshot {
    pub window_secs: i64,
    pub request_count: usize,
    pub success_count: usize,
    pub error_count: usize,
    pub stream_count: usize,
    pub retry_count: usize,
    pub avg_queue_ms: u64,
    pub p95_queue_ms: u64,
    pub avg_acquire_ms: u64,
    pub p95_acquire_ms: u64,
    pub avg_upstream_ms: u64,
    pub p50_upstream_ms: u64,
    pub p95_upstream_ms: u64,
    pub avg_total_ms: u64,
    pub p95_total_ms: u64,
    pub slow_models: Vec<ModelLatencySnapshot>,
    pub status_counts: Vec<StatusCountSnapshot>,
    pub credential_counts: Vec<CredentialCountSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLatencySnapshot {
    pub model: String,
    pub request_count: usize,
    pub avg_upstream_ms: u64,
    pub p95_upstream_ms: u64,
    pub avg_total_ms: u64,
    pub p95_total_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCountSnapshot {
    pub status: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialCountSnapshot {
    pub credential_id: u64,
    pub count: usize,
}

#[derive(Default)]
pub struct MetricsRecorder {
    samples: Mutex<VecDeque<RequestTimingSample>>,
}

impl MetricsRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, sample: RequestTimingSample) {
        let mut samples = self.samples.lock();
        samples.push_back(sample);
        prune_samples(&mut samples, Utc::now());
    }

    pub fn snapshot(&self) -> RuntimeMetricsSnapshot {
        let mut samples = self.samples.lock();
        let now = Utc::now();
        prune_samples(&mut samples, now);
        build_snapshot(samples.iter())
    }
}

pub fn duration_ms(duration: StdDuration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn prune_samples(samples: &mut VecDeque<RequestTimingSample>, now: DateTime<Utc>) {
    let cutoff = now - Duration::seconds(WINDOW_SECS);
    while samples
        .front()
        .map(|sample| sample.completed_at < cutoff)
        .unwrap_or(false)
    {
        samples.pop_front();
    }
    while samples.len() > MAX_SAMPLES {
        samples.pop_front();
    }
}

fn build_snapshot<'a>(
    samples: impl Iterator<Item = &'a RequestTimingSample>,
) -> RuntimeMetricsSnapshot {
    let samples: Vec<&RequestTimingSample> = samples.collect();
    let request_count = samples.len();
    let success_count = samples
        .iter()
        .filter(|sample| sample.outcome == UpstreamOutcome::Success)
        .count();
    let error_count = request_count.saturating_sub(success_count);
    let stream_count = samples.iter().filter(|sample| sample.stream).count();
    let retry_count = samples.iter().filter(|sample| sample.attempts > 1).count();

    let queue: Vec<u64> = samples.iter().map(|sample| sample.queue_ms).collect();
    let acquire: Vec<u64> = samples.iter().map(|sample| sample.acquire_ms).collect();
    let upstream: Vec<u64> = samples.iter().map(|sample| sample.upstream_ms).collect();
    let total: Vec<u64> = samples.iter().map(|sample| sample.total_ms).collect();

    let mut by_model: HashMap<String, Vec<&RequestTimingSample>> = HashMap::new();
    for sample in &samples {
        by_model
            .entry(sample.model.clone())
            .or_default()
            .push(*sample);
    }

    let mut slow_models: Vec<ModelLatencySnapshot> = by_model
        .into_iter()
        .map(|(model, samples)| {
            let upstream: Vec<u64> = samples.iter().map(|sample| sample.upstream_ms).collect();
            let total: Vec<u64> = samples.iter().map(|sample| sample.total_ms).collect();
            ModelLatencySnapshot {
                model,
                request_count: samples.len(),
                avg_upstream_ms: average(&upstream),
                p95_upstream_ms: percentile(&upstream, 95),
                avg_total_ms: average(&total),
                p95_total_ms: percentile(&total, 95),
            }
        })
        .collect();
    slow_models.sort_by(|a, b| b.p95_total_ms.cmp(&a.p95_total_ms));
    slow_models.truncate(5);

    let mut status_counts = count_statuses(&samples);
    status_counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.status.cmp(&b.status)));

    let mut credential_counts = count_credentials(&samples);
    credential_counts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.credential_id.cmp(&b.credential_id))
    });

    RuntimeMetricsSnapshot {
        window_secs: WINDOW_SECS,
        request_count,
        success_count,
        error_count,
        stream_count,
        retry_count,
        avg_queue_ms: average(&queue),
        p95_queue_ms: percentile(&queue, 95),
        avg_acquire_ms: average(&acquire),
        p95_acquire_ms: percentile(&acquire, 95),
        avg_upstream_ms: average(&upstream),
        p50_upstream_ms: percentile(&upstream, 50),
        p95_upstream_ms: percentile(&upstream, 95),
        avg_total_ms: average(&total),
        p95_total_ms: percentile(&total, 95),
        slow_models,
        status_counts,
        credential_counts,
    }
}

fn count_statuses(samples: &[&RequestTimingSample]) -> Vec<StatusCountSnapshot> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for sample in samples {
        let status = sample
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "network_error".to_string());
        *counts.entry(status).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(status, count)| StatusCountSnapshot { status, count })
        .collect()
}

fn count_credentials(samples: &[&RequestTimingSample]) -> Vec<CredentialCountSnapshot> {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for sample in samples {
        if let Some(credential_id) = sample.credential_id {
            *counts.entry(credential_id).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(credential_id, count)| CredentialCountSnapshot {
            credential_id,
            count,
        })
        .collect()
}

fn average(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.iter().sum::<u64>() / values.len() as u64
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len().saturating_sub(1)) * percentile) / 100;
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_groups_slow_models() {
        let recorder = MetricsRecorder::new();
        let now = Utc::now();
        recorder.record(RequestTimingSample {
            completed_at: now,
            model: "claude-sonnet-4.6".to_string(),
            stream: false,
            credential_id: Some(1),
            status: Some(200),
            outcome: UpstreamOutcome::Success,
            attempts: 1,
            queue_ms: 10,
            acquire_ms: 20,
            upstream_ms: 100,
            total_ms: 130,
        });
        recorder.record(RequestTimingSample {
            completed_at: now,
            model: "claude-opus-4.7".to_string(),
            stream: false,
            credential_id: Some(1),
            status: Some(200),
            outcome: UpstreamOutcome::Success,
            attempts: 1,
            queue_ms: 5,
            acquire_ms: 15,
            upstream_ms: 900,
            total_ms: 920,
        });

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.request_count, 2);
        assert_eq!(snapshot.success_count, 2);
        assert_eq!(snapshot.slow_models[0].model, "claude-opus-4.7");
        assert_eq!(snapshot.status_counts[0].status, "200");
        assert_eq!(snapshot.credential_counts[0].credential_id, 1);
    }
}
