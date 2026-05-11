use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::settings::RuntimeSettings;
use crate::kiro::store::KiroStore;
use crate::model::config::TlsBackend;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyBinding {
    pub credential_id: u64,
    pub provider: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub session_id: String,
    pub expires_at: Option<String>,
    pub status: String,
    pub egress_ip: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp_org: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_verified_at: Option<String>,
    pub verify_error: Option<String>,
    pub fail_count: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyBindingView {
    pub credential_id: u64,
    pub provider: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub session_id: String,
    pub expires_at: Option<String>,
    pub remaining_ms: u64,
    pub status: String,
    pub egress_ip: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp_org: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_verified_at: Option<String>,
    pub verify_error: Option<String>,
    pub fail_count: u32,
    pub has_password: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxySummary {
    pub enabled: bool,
    pub bound: usize,
    pub expiring_soon: usize,
    pub failed: usize,
    pub expired: usize,
    pub verifying: usize,
    pub rotating: usize,
    pub unbound: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyVerifyResult {
    pub egress_ip: String,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp_org: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicProxyActionResult {
    pub success: bool,
    pub binding: Option<DynamicProxyBindingView>,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct CredentialLite {
    pub id: u64,
    pub disabled: bool,
}

#[derive(Debug, Clone)]
struct WorkerCandidate {
    credential_id: u64,
    reason: &'static str,
    priority: u8,
}

#[derive(Debug, Deserialize)]
struct IpInfoResponse {
    ip: Option<String>,
    query: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    org: Option<String>,
    isp: Option<String>,
    #[serde(rename = "as")]
    as_name: Option<String>,
}

#[derive(Clone)]
pub struct DynamicProxyManager {
    store: KiroStore,
    tls_backend: TlsBackend,
}

impl DynamicProxyManager {
    pub fn new(store: KiroStore, tls_backend: TlsBackend) -> Self {
        Self { store, tls_backend }
    }

    pub fn effective_proxy(
        &self,
        credential_id: u64,
        manual_proxy: Option<ProxyConfig>,
    ) -> Option<ProxyConfig> {
        match self.active_binding_proxy(credential_id) {
            Ok(Some(proxy)) => Some(proxy),
            Ok(None) => manual_proxy,
            Err(err) => {
                tracing::warn!(
                    credential_id,
                    error = %err,
                    "读取动态代理绑定失败，回退到手动/全局代理"
                );
                manual_proxy
            }
        }
    }

    pub fn active_binding_proxy(&self, credential_id: u64) -> anyhow::Result<Option<ProxyConfig>> {
        let mut binding = match self.store.load_dynamic_proxy_binding(credential_id)? {
            Some(binding) => binding,
            None => return Ok(None),
        };
        if binding.status != "active" {
            return Ok(None);
        }
        if is_expired(&binding) {
            binding.status = "expired".to_string();
            binding.verify_error = Some("binding_expired".to_string());
            binding.updated_at = Some(Utc::now().to_rfc3339());
            self.store.save_dynamic_proxy_binding(&binding)?;
            return Ok(None);
        }
        Ok(Some(binding.proxy_config()))
    }

    pub fn binding_views(&self) -> anyhow::Result<Vec<DynamicProxyBindingView>> {
        Ok(self
            .store
            .load_dynamic_proxy_bindings()?
            .into_iter()
            .map(|binding| binding.view())
            .collect())
    }

    pub fn summary(
        &self,
        settings: &RuntimeSettings,
        credentials: &[CredentialLite],
    ) -> anyhow::Result<DynamicProxySummary> {
        let active_ids = credentials
            .iter()
            .filter(|credential| !credential.disabled)
            .map(|credential| credential.id)
            .collect::<HashSet<_>>();
        let bindings = self.store.load_dynamic_proxy_bindings()?;
        let mut bound = 0;
        let mut expiring_soon = 0;
        let mut failed = 0;
        let mut expired = 0;
        let mut verifying = 0;
        let mut rotating = 0;
        let now = Utc::now();
        for binding in bindings
            .iter()
            .filter(|binding| active_ids.contains(&binding.credential_id))
        {
            match binding.status.as_str() {
                "active" => {
                    if is_expired(binding) {
                        expired += 1;
                    } else {
                        bound += 1;
                        if expires_before(
                            binding,
                            now + Duration::milliseconds(
                                settings.dynamic_proxy_renew_before_ms as i64,
                            ),
                        ) {
                            expiring_soon += 1;
                        }
                    }
                }
                "failed" => failed += 1,
                "expired" => expired += 1,
                "verifying" => verifying += 1,
                "rotating" => rotating += 1,
                _ => {}
            }
        }
        let bound_ids = bindings
            .iter()
            .map(|binding| binding.credential_id)
            .collect::<HashSet<_>>();
        let unbound = active_ids.difference(&bound_ids).count();
        Ok(DynamicProxySummary {
            enabled: settings.dynamic_proxy_enabled,
            bound,
            expiring_soon,
            failed,
            expired,
            verifying,
            rotating,
            unbound,
        })
    }

    pub async fn bind(
        &self,
        credential_id: u64,
        settings: &RuntimeSettings,
        force: bool,
        rotating: bool,
    ) -> anyhow::Result<DynamicProxyActionResult> {
        settings.validate()?;
        if !settings.dynamic_proxy_enabled && !force {
            anyhow::bail!("动态代理未启用");
        }
        if settings.dynamic_proxy_host.trim().is_empty() {
            anyhow::bail!("动态代理 host 不能为空");
        }

        let max_retries = settings.dynamic_proxy_max_bind_retries.clamp(1, 20);
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=max_retries {
            let mut binding = generate_binding(credential_id, settings, rotating);
            self.store.save_dynamic_proxy_binding(&binding)?;
            match self.verify_proxy(binding.proxy_config(), settings).await {
                Ok(verified) => {
                    binding.status = "active".to_string();
                    binding.egress_ip = Some(verified.egress_ip);
                    binding.country = verified.country;
                    binding.region = verified.region;
                    binding.city = verified.city;
                    binding.isp_org = verified.isp_org;
                    binding.latency_ms = Some(verified.latency_ms);
                    binding.last_verified_at = Some(Utc::now().to_rfc3339());
                    binding.verify_error = None;
                    binding.fail_count = 0;
                    binding.updated_at = Some(Utc::now().to_rfc3339());
                    self.store.save_dynamic_proxy_binding(&binding)?;
                    tracing::info!(
                        credential_id,
                        egress_ip = binding.egress_ip.as_deref().unwrap_or("-"),
                        provider = %binding.provider,
                        "动态代理绑定成功"
                    );
                    return Ok(DynamicProxyActionResult {
                        success: true,
                        binding: Some(binding.view()),
                        attempts: attempt,
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    binding.status = "failed".to_string();
                    binding.expires_at = None;
                    binding.verify_error = Some(message.clone());
                    binding.fail_count = attempt;
                    binding.updated_at = Some(Utc::now().to_rfc3339());
                    self.store.save_dynamic_proxy_binding(&binding)?;
                    last_error = Some(anyhow::anyhow!(message));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("动态代理绑定失败")))
    }

    pub async fn rotate(
        &self,
        credential_id: u64,
        settings: &RuntimeSettings,
        force: bool,
    ) -> anyhow::Result<DynamicProxyActionResult> {
        self.bind(credential_id, settings, force, true).await
    }

    pub async fn verify(
        &self,
        credential_id: u64,
        settings: &RuntimeSettings,
        force: bool,
    ) -> anyhow::Result<DynamicProxyActionResult> {
        let mut binding = self
            .store
            .load_dynamic_proxy_binding(credential_id)?
            .ok_or_else(|| anyhow::anyhow!("凭据 #{} 未绑定动态代理", credential_id))?;
        if !force && !matches!(binding.status.as_str(), "active" | "failed" | "expired") {
            anyhow::bail!("当前动态代理状态为 {}，暂不可验证", binding.status);
        }
        match self.verify_proxy(binding.proxy_config(), settings).await {
            Ok(verified) => {
                binding.status = "active".to_string();
                binding.egress_ip = Some(verified.egress_ip);
                binding.country = verified.country;
                binding.region = verified.region;
                binding.city = verified.city;
                binding.isp_org = verified.isp_org;
                binding.latency_ms = Some(verified.latency_ms);
                binding.last_verified_at = Some(Utc::now().to_rfc3339());
                binding.verify_error = None;
                binding.updated_at = Some(Utc::now().to_rfc3339());
                self.store.save_dynamic_proxy_binding(&binding)?;
                Ok(DynamicProxyActionResult {
                    success: true,
                    binding: Some(binding.view()),
                    attempts: 1,
                })
            }
            Err(err) => {
                binding.status = "failed".to_string();
                binding.verify_error = Some(err.to_string());
                binding.fail_count = binding.fail_count.saturating_add(1);
                binding.updated_at = Some(Utc::now().to_rfc3339());
                self.store.save_dynamic_proxy_binding(&binding)?;
                Err(err)
            }
        }
    }

    pub fn clear(&self, credential_id: u64) -> anyhow::Result<bool> {
        self.store.delete_dynamic_proxy_binding(credential_id)
    }

    pub async fn mark_failure(
        self: &Arc<Self>,
        credential_id: u64,
        error: impl ToString,
        settings: &RuntimeSettings,
        auto_rebind: bool,
    ) -> anyhow::Result<Option<DynamicProxyBindingView>> {
        let mut binding = match self.store.load_dynamic_proxy_binding(credential_id)? {
            Some(binding) => binding,
            None => return Ok(None),
        };
        binding.status = "failed".to_string();
        binding.verify_error = Some(error.to_string());
        binding.fail_count = binding.fail_count.saturating_add(1);
        binding.updated_at = Some(Utc::now().to_rfc3339());
        self.store.save_dynamic_proxy_binding(&binding)?;
        let view = binding.view();
        if auto_rebind && settings.dynamic_proxy_enabled {
            let manager = self.clone();
            let settings = settings.clone();
            tokio::spawn(async move {
                if let Err(err) = manager.rotate(credential_id, &settings, false).await {
                    tracing::warn!(
                        credential_id,
                        error = %err,
                        "动态代理失败后自动换绑失败"
                    );
                }
            });
        }
        Ok(Some(view))
    }

    async fn verify_proxy(
        &self,
        proxy: ProxyConfig,
        settings: &RuntimeSettings,
    ) -> anyhow::Result<DynamicProxyVerifyResult> {
        let started_at = Instant::now();
        let client = build_client(Some(&proxy), 15, self.tls_backend)?;
        let response = client
            .get(settings.dynamic_proxy_verify_url.trim())
            .header("accept", "application/json,text/plain;q=0.9")
            .send()
            .await
            .with_context(|| "代理验证请求发送失败")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("代理验证失败: {} {}", status, body);
        }
        let info = normalize_verify_body(&body);
        if info.egress_ip.trim().is_empty() {
            anyhow::bail!("代理验证未返回出口 IP");
        }
        Ok(DynamicProxyVerifyResult {
            egress_ip: info.egress_ip,
            country: info.country,
            region: info.region,
            city: info.city,
            isp_org: info.isp_org,
            latency_ms: started_at.elapsed().as_millis() as u64,
        })
    }

    pub async fn run_maintenance_once(
        self: &Arc<Self>,
        settings: RuntimeSettings,
        credentials: Vec<CredentialLite>,
    ) -> anyhow::Result<(usize, usize)> {
        if !settings.dynamic_proxy_enabled {
            return Ok((0, 0));
        }
        let plan = self.worker_plan(&settings, &credentials)?;
        if plan.is_empty() {
            return Ok((0, 0));
        }
        let concurrency = settings.dynamic_proxy_worker_concurrency.max(1);
        let checked = plan.len();
        let manager = self.clone();
        let results = stream::iter(plan.into_iter().map(|item| {
            let manager = manager.clone();
            let settings = settings.clone();
            async move {
                let result = if item.reason == "unbound" {
                    manager
                        .bind(item.credential_id, &settings, false, false)
                        .await
                } else {
                    manager.rotate(item.credential_id, &settings, false).await
                };
                if let Err(err) = &result {
                    tracing::warn!(
                        credential_id = item.credential_id,
                        reason = item.reason,
                        error = %err,
                        "动态代理后台维护失败"
                    );
                }
                result.is_ok()
            }
        }))
        .buffer_unordered(concurrency)
        .collect::<Vec<bool>>()
        .await;
        let rebound = results.into_iter().filter(|ok| *ok).count();
        Ok((checked, rebound))
    }

    fn worker_plan(
        &self,
        settings: &RuntimeSettings,
        credentials: &[CredentialLite],
    ) -> anyhow::Result<Vec<WorkerCandidate>> {
        let active_ids = credentials
            .iter()
            .filter(|credential| !credential.disabled)
            .map(|credential| credential.id)
            .collect::<HashSet<_>>();
        let now = Utc::now();
        let renew_before =
            now + Duration::milliseconds(settings.dynamic_proxy_renew_before_ms as i64);
        let bindings = self.store.load_dynamic_proxy_bindings()?;
        let mut candidates = Vec::new();
        for binding in &bindings {
            if !active_ids.contains(&binding.credential_id) {
                continue;
            }
            match binding.status.as_str() {
                "failed" => candidates.push(WorkerCandidate {
                    credential_id: binding.credential_id,
                    reason: "failed",
                    priority: 1,
                }),
                "expired" => candidates.push(WorkerCandidate {
                    credential_id: binding.credential_id,
                    reason: "expired",
                    priority: 1,
                }),
                "active" if is_expired(binding) || expires_before(binding, renew_before) => {
                    candidates.push(WorkerCandidate {
                        credential_id: binding.credential_id,
                        reason: "expiring_soon",
                        priority: 2,
                    })
                }
                _ => {}
            }
        }
        if settings.dynamic_proxy_auto_bind_new_accounts {
            let bound = bindings
                .iter()
                .map(|binding| binding.credential_id)
                .collect::<HashSet<_>>();
            for credential_id in active_ids {
                if !bound.contains(&credential_id) {
                    candidates.push(WorkerCandidate {
                        credential_id,
                        reason: "unbound",
                        priority: 3,
                    });
                }
            }
        }
        candidates.sort_by_key(|item| (item.priority, item.credential_id));
        candidates.truncate(settings.dynamic_proxy_worker_batch_size);
        Ok(candidates)
    }
}

impl DynamicProxyBinding {
    pub fn proxy_config(&self) -> ProxyConfig {
        let url = format!("{}://{}:{}", self.protocol, self.host, self.port);
        let proxy = ProxyConfig::new(url);
        if self.username.is_empty() {
            proxy
        } else {
            proxy.with_auth(self.username.clone(), self.password.clone())
        }
    }

    pub fn view(&self) -> DynamicProxyBindingView {
        DynamicProxyBindingView {
            credential_id: self.credential_id,
            provider: self.provider.clone(),
            protocol: self.protocol.clone(),
            host: self.host.clone(),
            port: self.port,
            username: mask_username(&self.username),
            session_id: self.session_id.clone(),
            expires_at: self.expires_at.clone(),
            remaining_ms: remaining_ms(self.expires_at.as_deref()),
            status: self.status.clone(),
            egress_ip: self.egress_ip.clone(),
            country: self.country.clone(),
            region: self.region.clone(),
            city: self.city.clone(),
            isp_org: self.isp_org.clone(),
            latency_ms: self.latency_ms,
            last_verified_at: self.last_verified_at.clone(),
            verify_error: self.verify_error.clone(),
            fail_count: self.fail_count,
            has_password: !self.password.is_empty(),
        }
    }
}

pub fn is_proxy_error(err: &(dyn std::error::Error + 'static)) -> bool {
    let message = err.to_string();
    is_proxy_error_message(&message)
}

pub fn is_proxy_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "proxy",
        "socks",
        "tunnel",
        "407",
        "connection refused",
        "connection reset",
        "connection closed",
        "timed out",
        "timeout",
        "dns error",
        "failed to lookup address",
        "failed to connect",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn generate_binding(
    credential_id: u64,
    settings: &RuntimeSettings,
    rotating: bool,
) -> DynamicProxyBinding {
    let now = Utc::now();
    let session_id = random_session_id();
    let ttl = settings.dynamic_proxy_ttl_minutes.clamp(1, 24 * 60);
    let username = settings
        .dynamic_proxy_username_template
        .replace("{region}", &settings.dynamic_proxy_region)
        .replace("{state}", &settings.dynamic_proxy_state)
        .replace("{sid}", &session_id)
        .replace("{ttl}", &ttl.to_string());
    DynamicProxyBinding {
        credential_id,
        provider: settings.dynamic_proxy_provider.clone(),
        protocol: settings.dynamic_proxy_protocol.clone(),
        host: settings.dynamic_proxy_host.trim().to_string(),
        port: settings.dynamic_proxy_port,
        username,
        password: settings.dynamic_proxy_password.clone(),
        session_id,
        expires_at: Some((now + Duration::minutes(ttl as i64)).to_rfc3339()),
        status: if rotating { "rotating" } else { "verifying" }.to_string(),
        egress_ip: None,
        country: None,
        region: None,
        city: None,
        isp_org: None,
        latency_ms: None,
        last_verified_at: None,
        verify_error: None,
        fail_count: 0,
        created_at: Some(now.to_rfc3339()),
        updated_at: Some(now.to_rfc3339()),
    }
}

fn normalize_verify_body(body: &str) -> DynamicProxyVerifyResult {
    if let Ok(parsed) = serde_json::from_str::<IpInfoResponse>(body) {
        let isp_org = parsed.org.or(parsed.isp).or(parsed.as_name);
        return DynamicProxyVerifyResult {
            egress_ip: parsed.ip.or(parsed.query).unwrap_or_default(),
            country: parsed.country,
            region: parsed.region,
            city: parsed.city,
            isp_org,
            latency_ms: 0,
        };
    }
    DynamicProxyVerifyResult {
        egress_ip: body.trim().to_string(),
        country: None,
        region: None,
        city: None,
        isp_org: None,
        latency_ms: 0,
    }
}

fn is_expired(binding: &DynamicProxyBinding) -> bool {
    binding
        .expires_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|expires_at| expires_at <= Utc::now())
        .unwrap_or(false)
}

fn expires_before(binding: &DynamicProxyBinding, threshold: DateTime<Utc>) -> bool {
    binding
        .expires_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|expires_at| expires_at <= threshold)
        .unwrap_or(false)
}

fn remaining_ms(expires_at: Option<&str>) -> u64 {
    expires_at
        .and_then(parse_rfc3339)
        .map(|expires_at| {
            let now = Utc::now();
            if expires_at <= now {
                0
            } else {
                (expires_at - now).num_milliseconds().max(0) as u64
            }
        })
        .unwrap_or(0)
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn random_session_id() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect()
}

fn mask_username(username: &str) -> String {
    if username.is_empty() {
        return String::new();
    }
    if username.len() <= 12 {
        return "***".to_string();
    }
    format!("{}...{}", &username[..10], &username[username.len() - 6..])
}

pub fn spawn_dynamic_proxy_worker(
    manager: Arc<DynamicProxyManager>,
    token_manager: Arc<crate::kiro::token_manager::MultiTokenManager>,
) {
    tokio::spawn(async move {
        loop {
            let settings = token_manager.runtime_settings();
            let interval_ms = settings.dynamic_proxy_worker_interval_ms.max(1_000);
            if settings.dynamic_proxy_enabled {
                let credentials = token_manager.credential_lites();
                match manager.run_maintenance_once(settings, credentials).await {
                    Ok((checked, rebound)) if checked > 0 || rebound > 0 => {
                        tracing::info!(checked, rebound, "动态代理后台维护完成");
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!(error = %err, "动态代理后台维护失败"),
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::Config;

    fn settings() -> RuntimeSettings {
        let mut settings = RuntimeSettings::from_config(&Config::default());
        settings.dynamic_proxy_enabled = true;
        settings.dynamic_proxy_host = "proxy.example.com".to_string();
        settings.dynamic_proxy_port = 1234;
        settings.dynamic_proxy_password = "secret".to_string();
        settings.dynamic_proxy_username_template =
            "user-region-{region}-st-{state}-sid-{sid}-t-{ttl}".to_string();
        settings
    }

    #[test]
    fn generated_binding_renders_template() {
        let settings = settings();
        let binding = generate_binding(7, &settings, false);
        assert_eq!(binding.credential_id, 7);
        assert_eq!(binding.status, "verifying");
        assert!(binding.username.contains("region-US"));
        assert!(binding.username.contains("-sid-"));
        assert!(binding.username.ends_with("-t-120"));
    }

    #[test]
    fn proxy_error_detection_matches_common_errors() {
        assert!(is_proxy_error_message("proxy authentication 407"));
        assert!(is_proxy_error_message("SOCKS5 tunnel failed"));
        assert!(is_proxy_error_message("connection timed out"));
        assert!(!is_proxy_error_message("429 Too Many Requests"));
    }
}
