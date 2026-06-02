//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, timeout};

use crate::anthropic::VirtualCacheUsageManager;
use crate::http_client::build_client;
use crate::kiro::dynamic_proxy::DynamicProxyManager;
use crate::kiro::endpoint::{endpoint_api_url, endpoint_label, normalize_endpoint_name};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model_cooldown::ModelCooldownManager;
use crate::kiro::provider::KiroProvider;
use crate::kiro::settings::CredentialPolicy;
use crate::kiro::token_manager::MultiTokenManager;
use crate::metrics::MetricsRecorder;
use crate::runtime::RuntimeLimiter;

use super::error::AdminServiceError;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, BatchCredentialIdsRequest,
    BatchCredentialPolicyRequest, CredentialStatusItem, CredentialTestRequest,
    CredentialTestResponse, CredentialsStatusResponse, DynamicProxyActionResponse,
    DynamicProxyBatchActionResponse, DynamicProxyBindingsResponse, EndpointConfigResponse,
    EndpointLatencyResponse, EndpointOption, ExportCredentialsRequest, ExportCredentialsResponse,
    LoadBalancingModeResponse, RuntimeCredentialStatus, RuntimeSettingsResponse,
    RuntimeStatusResponse, SetCredentialPolicyRequest, SetLoadBalancingModeRequest,
    SetRuntimeSettingsRequest,
};

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;

/// 缓存的余额条目（含时间戳）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBalance {
    /// 缓存时间（Unix 秒）
    cached_at: f64,
    /// 缓存的余额数据
    data: BalanceResponse,
}

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    provider: Arc<KiroProvider>,
    runtime_limiter: Arc<RuntimeLimiter>,
    metrics: Arc<MetricsRecorder>,
    model_cooldowns: Arc<ModelCooldownManager>,
    dynamic_proxy: Arc<DynamicProxyManager>,
    virtual_cache_usage: Arc<VirtualCacheUsageManager>,
    balance_cache: Mutex<HashMap<u64, CachedBalance>>,
    cache_path: Option<PathBuf>,
    /// 已注册的端点名称集合（用于 add_credential 校验）
    known_endpoints: HashSet<String>,
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        provider: Arc<KiroProvider>,
        runtime_limiter: Arc<RuntimeLimiter>,
        metrics: Arc<MetricsRecorder>,
        model_cooldowns: Arc<ModelCooldownManager>,
        dynamic_proxy: Arc<DynamicProxyManager>,
        virtual_cache_usage: Arc<VirtualCacheUsageManager>,
        known_endpoints: impl IntoIterator<Item = String>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));

        let balance_cache = Self::load_balance_cache_from(&cache_path);

        Self {
            token_manager,
            provider,
            runtime_limiter,
            metrics,
            model_cooldowns,
            dynamic_proxy,
            virtual_cache_usage,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            known_endpoints: known_endpoints.into_iter().collect(),
        }
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();
        let default_endpoint = self.token_manager.default_endpoint();
        let dynamic_bindings = self.dynamic_proxy_bindings_map();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| CredentialStatusItem {
                id: entry.id,
                priority: entry.priority,
                disabled: entry.disabled,
                failure_count: entry.failure_count,
                is_current: entry.id == snapshot.current_id,
                expires_at: entry.expires_at,
                auth_method: entry.auth_method,
                provider: entry.provider,
                has_profile_arn: entry.has_profile_arn,
                refresh_token_hash: entry.refresh_token_hash,
                api_key_hash: entry.api_key_hash,
                masked_api_key: entry.masked_api_key,
                email: entry.email,
                subscription_title: entry.subscription_title,
                success_count: entry.success_count,
                last_used_at: entry.last_used_at.clone(),
                has_proxy: entry.has_proxy,
                proxy_url: entry.proxy_url,
                refresh_failure_count: entry.refresh_failure_count,
                disabled_reason: entry.disabled_reason,
                endpoint: entry.endpoint.unwrap_or_else(|| default_endpoint.clone()),
                allow_overage: entry.allow_overage,
                overage_weight: entry.overage_weight,
                overage_stopped: entry.overage_stopped,
                usage_current: entry.usage_current,
                usage_limit: entry.usage_limit,
                usage_percentage: entry.usage_percentage,
                is_over_usage_limit: entry.is_over_usage_limit,
                in_flight: entry.in_flight,
                max_concurrent: entry.max_concurrent,
                max_concurrent_override: entry.max_concurrent_override,
                rpm_override: entry.rpm_override,
                turbo_mode: entry.turbo_mode,
                turbo_fanout: entry.turbo_fanout,
                effective_rpm: entry.effective_rpm,
                uses_default_policy: entry.uses_default_policy,
                cooldown_until: entry.cooldown_until,
                is_cooling_down: entry.is_cooling_down,
                available_for_dispatch: entry.available_for_dispatch,
                session_affinity_bindings: entry.session_affinity_bindings,
                dynamic_proxy: dynamic_bindings.get(&entry.id).cloned(),
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            current_id: snapshot.current_id,
            credentials,
        }
    }

    /// 获取运行时状态
    pub fn get_runtime_status(&self) -> RuntimeStatusResponse {
        let snapshot = self.token_manager.snapshot();
        let settings = self.token_manager.runtime_settings();
        let credentials_lite = self.token_manager.credential_lites();
        let dynamic_proxy_summary = self
            .dynamic_proxy
            .summary(&settings, &credentials_lite)
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "获取动态代理摘要失败");
                crate::kiro::dynamic_proxy::DynamicProxySummary {
                    enabled: settings.dynamic_proxy_enabled,
                    bound: 0,
                    expiring_soon: 0,
                    failed: 0,
                    expired: 0,
                    verifying: 0,
                    rotating: 0,
                    unbound: 0,
                }
            });
        let dynamic_bindings = self.dynamic_proxy_bindings_map();
        let virtual_cache_reuse = self
            .virtual_cache_usage
            .reuse_snapshot(settings.target_cache_reuse_ratio);
        RuntimeStatusResponse {
            default_endpoint: snapshot.default_endpoint.clone(),
            endpoints: self.endpoint_options(&snapshot.default_endpoint),
            global_in_flight: snapshot.global_in_flight,
            global_max_concurrent: snapshot.global_max_concurrent,
            per_account_default_max_concurrent: snapshot.per_account_default_max_concurrent,
            global_rpm: snapshot.global_rpm,
            per_account_default_rpm: snapshot.per_account_default_rpm,
            queue_depth: snapshot.queue_depth,
            queue_max_size: snapshot.queue_max_size,
            queue_timeout_ms: snapshot.queue_timeout_ms,
            rate_limit_cooldown_ms: snapshot.rate_limit_cooldown_ms,
            transient_cooldown_ms: snapshot.transient_cooldown_ms,
            max_retry_accounts: snapshot.max_retry_accounts,
            allow_over_usage: snapshot.allow_over_usage,
            model_capacity_cooldown_ms: snapshot.model_capacity_cooldown_ms,
            same_account_retry_rules: snapshot.same_account_retry_rules,
            token_auto_refresh_enabled: snapshot.token_auto_refresh_enabled,
            token_auto_refresh_interval_secs: snapshot.token_auto_refresh_interval_secs,
            token_auto_refresh_window_secs: snapshot.token_auto_refresh_window_secs,
            session_affinity_enabled: snapshot.session_affinity_enabled,
            session_affinity_ttl_secs: snapshot.session_affinity_ttl_secs,
            opus47_plain_stabilization_mode: snapshot.opus47_plain_stabilization_mode,
            opus47_antml_probe_compat: snapshot.opus47_antml_probe_compat,
            opus47_clean_probe_mode: snapshot.opus47_clean_probe_mode,
            opus47_detection_profile: snapshot.opus47_detection_profile,
            opus47_signed_thinking_preservation: snapshot.opus47_signed_thinking_preservation,
            opus47_short_thinking_experiment: snapshot.opus47_short_thinking_experiment,
            opus47_diagnostics_enabled: snapshot.opus47_diagnostics_enabled,
            opus47_raw_debug_enabled: snapshot.opus47_raw_debug_enabled,
            opus47_raw_debug_max_chars: snapshot.opus47_raw_debug_max_chars,
            opus46_detection_profile: snapshot.opus46_detection_profile,
            opus46_antml_probe_compat: snapshot.opus46_antml_probe_compat,
            opus46_diagnostics_enabled: snapshot.opus46_diagnostics_enabled,
            opus46_raw_debug_enabled: snapshot.opus46_raw_debug_enabled,
            opus46_raw_debug_max_chars: snapshot.opus46_raw_debug_max_chars,
            sonnet46_detection_profile: snapshot.sonnet46_detection_profile,
            sonnet46_antml_probe_compat: snapshot.sonnet46_antml_probe_compat,
            sonnet46_diagnostics_enabled: snapshot.sonnet46_diagnostics_enabled,
            sonnet46_raw_debug_enabled: snapshot.sonnet46_raw_debug_enabled,
            sonnet46_raw_debug_max_chars: snapshot.sonnet46_raw_debug_max_chars,
            prompt_dump_enabled: snapshot.prompt_dump_enabled,
            prompt_dump_dir: snapshot.prompt_dump_dir,
            prompt_dump_max_bytes: snapshot.prompt_dump_max_bytes,
            prompt_dump_models: snapshot.prompt_dump_models,
            compat_usage_shape: snapshot.compat_usage_shape,
            compat_thinking_model: snapshot.compat_thinking_model,
            compat_models_shape: snapshot.compat_models_shape,
            load_balancing_mode: snapshot.load_balancing_mode,
            virtual_cache_reuse,
            total_credentials: snapshot.total,
            available_credentials: snapshot.available,
            dispatch_available_credentials: snapshot.dispatch_available,
            cooling_down_credentials: snapshot.cooling_down,
            session_affinity_bindings: snapshot.session_affinity_bindings,
            request_metrics: self.metrics.snapshot(),
            model_cooldowns: self.model_cooldowns.snapshot(),
            dynamic_proxy: dynamic_proxy_summary,
            credentials: snapshot
                .entries
                .into_iter()
                .map(|entry| RuntimeCredentialStatus {
                    id: entry.id,
                    in_flight: entry.in_flight,
                    max_concurrent: entry.max_concurrent,
                    max_concurrent_override: entry.max_concurrent_override,
                    rpm_override: entry.rpm_override,
                    turbo_mode: entry.turbo_mode,
                    turbo_fanout: entry.turbo_fanout,
                    effective_rpm: entry.effective_rpm,
                    uses_default_policy: entry.uses_default_policy,
                    cooldown_until: entry.cooldown_until,
                    is_cooling_down: entry.is_cooling_down,
                    available_for_dispatch: entry.available_for_dispatch,
                    session_affinity_bindings: entry.session_affinity_bindings,
                    dynamic_proxy: dynamic_bindings.get(&entry.id).cloned(),
                })
                .collect(),
        }
    }

    fn endpoint_options(&self, default_endpoint: &str) -> Vec<EndpointOption> {
        let mut names: Vec<&str> = self.known_endpoints.iter().map(|s| s.as_str()).collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let api_url =
                    endpoint_api_url(name, self.token_manager.config()).unwrap_or_default();
                EndpointOption {
                    name: name.to_string(),
                    label: endpoint_label(name).unwrap_or(name).to_string(),
                    api_url,
                    is_default: name == default_endpoint,
                }
            })
            .collect()
    }

    pub fn get_endpoints(&self) -> EndpointConfigResponse {
        let default_endpoint = self.token_manager.default_endpoint();
        EndpointConfigResponse {
            endpoints: self.endpoint_options(&default_endpoint),
            default_endpoint,
        }
    }

    fn dynamic_proxy_bindings_map(
        &self,
    ) -> HashMap<u64, crate::kiro::dynamic_proxy::DynamicProxyBindingView> {
        self.dynamic_proxy
            .binding_views()
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "读取动态代理绑定失败");
                Vec::new()
            })
            .into_iter()
            .map(|binding| (binding.credential_id, binding))
            .collect()
    }

    /// 设置凭据禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminServiceError> {
        // 先获取当前凭据 ID，用于判断是否需要切换
        let snapshot = self.token_manager.snapshot();
        let current_id = snapshot.current_id;

        self.token_manager
            .set_disabled(id, disabled)
            .map_err(|e| self.classify_error(e, id))?;

        // 只有禁用的是当前凭据时才尝试切换到下一个
        if disabled && id == current_id {
            let _ = self.token_manager.switch_to_next();
        }
        Ok(())
    }

    /// 设置凭据优先级
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_priority(id, priority)
            .map_err(|e| self.classify_error(e, id))
    }

    pub fn get_runtime_settings(&self) -> RuntimeSettingsResponse {
        self.token_manager.runtime_settings()
    }

    pub fn set_runtime_settings(
        &self,
        req: SetRuntimeSettingsRequest,
    ) -> Result<RuntimeSettingsResponse, AdminServiceError> {
        req.validate()
            .map_err(|e| AdminServiceError::InvalidCredential(e.to_string()))?;
        self.token_manager
            .update_runtime_settings(req.clone())
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        self.runtime_limiter.notify_capacity_available();
        Ok(self.token_manager.runtime_settings())
    }

    pub fn get_dynamic_proxy_bindings(&self) -> DynamicProxyBindingsResponse {
        DynamicProxyBindingsResponse {
            bindings: self.dynamic_proxy_bindings_map().into_values().collect(),
        }
    }

    pub async fn test_credential(
        &self,
        id: u64,
        req: CredentialTestRequest,
    ) -> Result<CredentialTestResponse, AdminServiceError> {
        let model = req.model.trim();
        if model.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "测试模型不能为空".to_string(),
            ));
        }
        let prompt = req.prompt.unwrap_or_else(|| "hi".to_string());
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "测试消息不能为空".to_string(),
            ));
        }

        let result = timeout(
            Duration::from_secs(60),
            self.provider.test_credential_message(id, model, prompt),
        )
        .await
        .map_err(|_| AdminServiceError::UpstreamError("账号测试超时".to_string()))?
        .map_err(|err| self.classify_test_error(err, id))?;

        Ok(CredentialTestResponse {
            credential_id: result.credential_id,
            model: result.model,
            prompt: result.prompt,
            response_text: result.response_text,
            status: result.status,
            latency_ms: result.latency_ms,
            endpoint: result.endpoint,
            api_region: result.api_region,
        })
    }

    pub async fn test_endpoint_latency(
        &self,
        name: String,
    ) -> Result<EndpointLatencyResponse, AdminServiceError> {
        let endpoint = normalize_endpoint_name(&name);
        if !self.known_endpoints.contains(&endpoint) {
            return Err(AdminServiceError::InvalidCredential(format!(
                "未知端点 \"{}\"",
                name
            )));
        }

        let api_url = endpoint_api_url(&endpoint, self.token_manager.config())
            .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;
        let label = endpoint_label(&endpoint).unwrap_or(&endpoint).to_string();
        let client = build_client(
            self.token_manager.global_proxy().as_ref(),
            10,
            self.token_manager.config().tls_backend,
        )
        .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;

        let started_at = Instant::now();
        let result = timeout(Duration::from_secs(10), client.get(&api_url).send()).await;
        let latency_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

        match result {
            Ok(Ok(response)) => Ok(EndpointLatencyResponse {
                endpoint,
                label,
                api_url,
                network_ok: true,
                status: Some(response.status().as_u16()),
                latency_ms,
                error: None,
            }),
            Ok(Err(err)) => Ok(EndpointLatencyResponse {
                endpoint,
                label,
                api_url,
                network_ok: false,
                status: None,
                latency_ms,
                error: Some(err.to_string()),
            }),
            Err(_) => Ok(EndpointLatencyResponse {
                endpoint,
                label,
                api_url,
                network_ok: false,
                status: None,
                latency_ms,
                error: Some("latency probe timed out after 10s".to_string()),
            }),
        }
    }

    pub async fn bind_dynamic_proxy(
        &self,
        id: u64,
    ) -> Result<DynamicProxyActionResponse, AdminServiceError> {
        self.ensure_credential_exists(id)?;
        let settings = self.token_manager.runtime_settings();
        let result = self
            .dynamic_proxy
            .bind(id, &settings, true, false)
            .await
            .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;
        Ok(DynamicProxyActionResponse {
            success: result.success,
            binding: result.binding,
            attempts: result.attempts,
        })
    }

    pub async fn rotate_dynamic_proxy(
        &self,
        id: u64,
    ) -> Result<DynamicProxyActionResponse, AdminServiceError> {
        self.ensure_credential_exists(id)?;
        let settings = self.token_manager.runtime_settings();
        let result = self
            .dynamic_proxy
            .rotate(id, &settings, true)
            .await
            .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;
        Ok(DynamicProxyActionResponse {
            success: result.success,
            binding: result.binding,
            attempts: result.attempts,
        })
    }

    pub async fn verify_dynamic_proxy(
        &self,
        id: u64,
    ) -> Result<DynamicProxyActionResponse, AdminServiceError> {
        self.ensure_credential_exists(id)?;
        let settings = self.token_manager.runtime_settings();
        let result = self
            .dynamic_proxy
            .verify(id, &settings, true)
            .await
            .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;
        Ok(DynamicProxyActionResponse {
            success: result.success,
            binding: result.binding,
            attempts: result.attempts,
        })
    }

    pub fn clear_dynamic_proxy(&self, id: u64) -> Result<(), AdminServiceError> {
        self.ensure_credential_exists(id)?;
        self.dynamic_proxy
            .clear(id)
            .map_err(|err| AdminServiceError::InternalError(err.to_string()))?;
        Ok(())
    }

    pub async fn dynamic_proxy_batch_action(
        &self,
        action: &str,
        req: BatchCredentialIdsRequest,
    ) -> Result<DynamicProxyBatchActionResponse, AdminServiceError> {
        if req.ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择要操作的凭据".to_string(),
            ));
        }
        let settings = self.token_manager.runtime_settings();
        let requested = req.ids.len();
        let mut succeeded = 0usize;
        let mut errors = Vec::new();
        for id in req.ids {
            let result = match action {
                "bind" => self
                    .dynamic_proxy
                    .bind(id, &settings, true, false)
                    .await
                    .map(|_| ()),
                "rotate" => self
                    .dynamic_proxy
                    .rotate(id, &settings, true)
                    .await
                    .map(|_| ()),
                "verify" => self
                    .dynamic_proxy
                    .verify(id, &settings, true)
                    .await
                    .map(|_| ()),
                "clear" => self.dynamic_proxy.clear(id).map(|_| ()),
                _ => Err(anyhow::anyhow!("未知动态代理操作: {}", action)),
            };
            match result {
                Ok(_) => succeeded += 1,
                Err(err) => errors.push(format!("#{}: {}", id, err)),
            }
        }
        Ok(DynamicProxyBatchActionResponse {
            success: errors.is_empty(),
            requested,
            succeeded,
            failed: errors.len(),
            errors,
        })
    }

    fn ensure_credential_exists(&self, id: u64) -> Result<(), AdminServiceError> {
        if self
            .token_manager
            .snapshot()
            .entries
            .into_iter()
            .any(|entry| entry.id == id)
        {
            Ok(())
        } else {
            Err(AdminServiceError::NotFound { id })
        }
    }

    pub fn set_policy(
        &self,
        id: u64,
        req: SetCredentialPolicyRequest,
    ) -> Result<(), AdminServiceError> {
        let policy = CredentialPolicy {
            max_concurrent_override: req.max_concurrent_override,
            rpm_override: req.rpm_override,
            turbo_mode: req.turbo_mode,
            turbo_fanout: req.turbo_fanout,
            allow_overage: req.allow_overage,
            overage_weight: req.overage_weight,
        };
        self.token_manager
            .set_policy(id, policy.clone())
            .map_err(|e| self.classify_error(e, id))?;
        tracing::info!(
            credential_id = id,
            turbo_mode = policy.effective_turbo_mode(),
            turbo_fanout = policy.effective_turbo_fanout(),
            "admin_credential_policy_update"
        );
        Ok(())
    }

    pub fn set_policy_batch(
        &self,
        req: BatchCredentialPolicyRequest,
    ) -> Result<(), AdminServiceError> {
        if req.ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择要修改的凭据".to_string(),
            ));
        }
        let policy = CredentialPolicy {
            max_concurrent_override: req.max_concurrent_override,
            rpm_override: req.rpm_override,
            turbo_mode: req.turbo_mode,
            turbo_fanout: req.turbo_fanout,
            allow_overage: req.allow_overage,
            overage_weight: req.overage_weight,
        };
        self.token_manager
            .set_policy_batch(&req.ids, policy.clone())
            .map_err(|e| AdminServiceError::InvalidCredential(e.to_string()))?;
        tracing::info!(
            credential_ids = ?req.ids,
            turbo_mode = policy.effective_turbo_mode(),
            turbo_fanout = policy.effective_turbo_fanout(),
            "admin_credential_policy_batch_update"
        );
        Ok(())
    }

    pub fn clear_cooldown(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .clear_cooldown(id)
            .map_err(|e| self.classify_error(e, id))
    }

    pub fn clear_cooldown_batch(
        &self,
        req: BatchCredentialIdsRequest,
    ) -> Result<(), AdminServiceError> {
        if req.ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择要清除冷却的凭据".to_string(),
            ));
        }
        self.token_manager
            .clear_cooldown_batch(&req.ids)
            .map_err(|e| AdminServiceError::InvalidCredential(e.to_string()))
    }

    /// 重置失败计数并重新启用
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .reset_and_enable(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 获取凭据余额（带缓存）
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        // 先查缓存
        {
            let cache = self.balance_cache.lock();
            if let Some(cached) = cache.get(&id) {
                let now = Utc::now().timestamp() as f64;
                if (now - cached.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    tracing::debug!("凭据 #{} 余额命中缓存", id);
                    if let Err(err) = self.token_manager.update_usage_snapshot(
                        id,
                        cached.data.current_usage,
                        cached.data.usage_limit,
                    ) {
                        tracing::warn!(credential_id = id, error = %err, "缓存余额同步到账号额度快照失败");
                    }
                    return Ok(cached.data.clone());
                }
            }
        }

        // 缓存未命中或已过期，从上游获取
        let balance = self.fetch_balance(id).await?;

        // 更新缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.insert(
                id,
                CachedBalance {
                    cached_at: Utc::now().timestamp() as f64,
                    data: balance.clone(),
                },
            );
        }
        self.save_balance_cache();

        Ok(balance)
    }

    /// 从上游获取余额（无缓存）
    async fn fetch_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        let usage = self
            .token_manager
            .get_usage_limits_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = usage.current_usage();
        let usage_limit = usage.usage_limit();
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            current_usage / usage_limit * 100.0
        } else {
            0.0
        };
        if let Err(err) = self
            .token_manager
            .update_usage_snapshot(id, current_usage, usage_limit)
        {
            tracing::warn!(credential_id = id, error = %err, "更新账号额度快照失败");
        }

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
        })
    }

    /// 添加新凭据
    pub async fn add_credential(
        &self,
        req: AddCredentialRequest,
    ) -> Result<AddCredentialResponse, AdminServiceError> {
        // 校验端点名：未指定则默认合法，指定则必须已注册
        let endpoint = req.endpoint.as_deref().map(normalize_endpoint_name);
        if let Some(ref name) = endpoint {
            if !self.known_endpoints.contains(name) {
                let mut known: Vec<&str> =
                    self.known_endpoints.iter().map(|s| s.as_str()).collect();
                known.sort();
                return Err(AdminServiceError::InvalidCredential(format!(
                    "未知端点 \"{}\"，已注册端点: {:?}",
                    name, known
                )));
            }
        }

        // 构建凭据对象
        let email = req.email.clone();
        let region = req.region.or_else(|| {
            let is_enterprise = req
                .provider
                .as_deref()
                .is_some_and(|provider| provider.eq_ignore_ascii_case("Enterprise"));
            if is_enterprise && req.profile_arn.as_deref().is_none_or(str::is_empty) {
                req.auth_region.clone()
            } else {
                None
            }
        });
        let new_cred = KiroCredentials {
            id: None,
            access_token: req.access_token,
            refresh_token: req.refresh_token,
            profile_arn: req.profile_arn,
            provider: req.provider,
            expires_at: req.expires_at,
            auth_method: Some(req.auth_method),
            client_id: req.client_id,
            client_secret: req.client_secret,
            priority: req.priority,
            region,
            auth_region: req.auth_region,
            api_region: req.api_region,
            machine_id: req.machine_id,
            email: req.email,
            subscription_title: req.subscription_title, // 后续余额查询会自动刷新
            proxy_url: req.proxy_url,
            proxy_username: req.proxy_username,
            proxy_password: req.proxy_password,
            disabled: false, // 新添加的凭据默认启用
            kiro_api_key: req.kiro_api_key,
            endpoint,
            allow_overage: req.allow_overage,
            overage_weight: req.overage_weight,
            usage_current: req.usage_current.unwrap_or(0.0).max(0.0),
            usage_limit: req.usage_limit.unwrap_or(0.0).max(0.0),
            overage_stopped: req.overage_stopped,
        };

        // 调用 token_manager 添加凭据
        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| self.classify_add_error(e))?;

        // 主动获取订阅等级，避免首次请求时 Free 账号绕过 Opus 模型过滤
        if let Err(e) = self.token_manager.get_usage_limits_for(credential_id).await {
            tracing::warn!("添加凭据后获取订阅等级失败（不影响凭据添加）: {}", e);
        }

        Ok(AddCredentialResponse {
            success: true,
            message: format!("凭据添加成功，ID: {}", credential_id),
            credential_id,
            email,
        })
    }

    /// 导出指定凭据的明文 JSON 数据
    pub fn export_credentials(
        &self,
        req: ExportCredentialsRequest,
    ) -> Result<ExportCredentialsResponse, AdminServiceError> {
        if req.ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择要导出的凭据".to_string(),
            ));
        }

        let credentials = self
            .token_manager
            .export_credentials_by_ids(&req.ids)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("不存在") {
                    AdminServiceError::InvalidCredential(msg)
                } else {
                    AdminServiceError::InternalError(msg)
                }
            })?;

        Ok(ExportCredentialsResponse {
            count: credentials.len(),
            credentials,
        })
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .delete_credential(id)
            .map_err(|e| self.classify_delete_error(e, id))?;

        // 清理已删除凭据的余额缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.remove(&id);
        }
        self.save_balance_cache();

        Ok(())
    }

    /// 获取负载均衡模式
    pub fn get_load_balancing_mode(&self) -> LoadBalancingModeResponse {
        LoadBalancingModeResponse {
            mode: self.token_manager.get_load_balancing_mode(),
        }
    }

    /// 设置负载均衡模式
    pub fn set_load_balancing_mode(
        &self,
        req: SetLoadBalancingModeRequest,
    ) -> Result<LoadBalancingModeResponse, AdminServiceError> {
        // 验证模式值
        if req.mode != "priority" && req.mode != "balanced" {
            return Err(AdminServiceError::InvalidCredential(
                "mode 必须是 'priority' 或 'balanced'".to_string(),
            ));
        }

        self.token_manager
            .set_load_balancing_mode(req.mode.clone())
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        self.runtime_limiter.notify_capacity_available();

        Ok(LoadBalancingModeResponse { mode: req.mode })
    }

    /// 强制刷新指定凭据的 Token
    pub async fn force_refresh_token(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .force_refresh_token_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))
    }

    // ============ 余额缓存持久化 ============

    fn load_balance_cache_from(cache_path: &Option<PathBuf>) -> HashMap<u64, CachedBalance> {
        let path = match cache_path {
            Some(p) => p,
            None => return HashMap::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        // 文件中使用字符串 key 以兼容 JSON 格式
        let map: HashMap<String, CachedBalance> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析余额缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };

        let now = Utc::now().timestamp() as f64;
        map.into_iter()
            .filter_map(|(k, v)| {
                let id = k.parse::<u64>().ok()?;
                // 丢弃超过 TTL 的条目
                if (now - v.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    Some((id, v))
                } else {
                    None
                }
            })
            .collect()
    }

    fn save_balance_cache(&self) {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return,
        };

        // 持有锁期间完成序列化和写入，防止并发损坏
        let cache = self.balance_cache.lock();
        let map: HashMap<String, &CachedBalance> =
            cache.iter().map(|(k, v)| (k.to_string(), v)).collect();

        match serde_json::to_string_pretty(&map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("保存余额缓存失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("序列化余额缓存失败: {}", e),
        }
    }

    // ============ 错误分类 ============

    /// 分类简单操作错误（set_disabled, set_priority, reset_and_enable）
    fn classify_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    fn classify_test_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("不支持的测试模型") {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::UpstreamError(msg)
        }
    }

    /// 分类余额查询错误（可能涉及上游 API 调用）
    fn classify_balance_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();

        // 1. 凭据不存在
        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        // 2. API Key 凭据不支持刷新：客户端请求错误，映射为 400
        if msg.contains("API Key 凭据不支持刷新") {
            return AdminServiceError::InvalidCredential(msg);
        }

        // 3. 上游服务错误特征：HTTP 响应错误或网络错误
        let is_upstream_error =
            // HTTP 响应错误（来自 refresh_*_token 的错误消息）
            msg.contains("凭证已过期或无效") ||
            msg.contains("权限不足") ||
            msg.contains("已被限流") ||
            msg.contains("服务器错误") ||
            msg.contains("Token 刷新失败") ||
            msg.contains("获取使用额度失败") ||
            msg.contains("认证失败") ||
            msg.contains("Invalid profileArn") ||
            msg.contains("暂时不可用") ||
            // 网络错误（reqwest 错误）
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out");

        if is_upstream_error {
            AdminServiceError::UpstreamError(msg)
        } else {
            // 4. 默认归类为内部错误（本地验证失败、配置错误等）
            // 包括：缺少 refreshToken、refreshToken 已被截断、无法生成 machineId 等
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类添加凭据错误
    fn classify_add_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();

        // 凭据验证失败（refreshToken 无效、格式错误等）
        let is_invalid_credential = msg.contains("缺少 refreshToken")
            || msg.contains("refreshToken 为空")
            || msg.contains("refreshToken 已被截断")
            || msg.contains("凭据已存在")
            || msg.contains("refreshToken 重复")
            || msg.contains("kiroApiKey 重复")
            || msg.contains("缺少 kiroApiKey")
            || msg.contains("kiroApiKey 为空")
            || msg.contains("凭证已过期或无效")
            || msg.contains("权限不足")
            || msg.contains("已被限流");

        if is_invalid_credential {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("error trying to connect")
            || msg.contains("connection")
            || msg.contains("timeout")
        {
            AdminServiceError::UpstreamError(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("只能删除已禁用的凭据") || msg.contains("请先禁用凭据")
        {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }
}
