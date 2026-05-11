//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, BatchCredentialIdsRequest, BatchCredentialPolicyRequest,
        ExportCredentialsRequest, SetCredentialPolicyRequest, SetDisabledRequest,
        SetLoadBalancingModeRequest, SetPriorityRequest, SetRuntimeSettingsRequest,
        SuccessResponse,
    },
};

/// GET /api/admin/runtime
/// 获取运行时状态
pub async fn get_runtime_status(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_runtime_status();
    Json(response)
}

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// GET /api/admin/settings/runtime
/// 获取运行时调度配置
pub async fn get_runtime_settings(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_runtime_settings();
    Json(response)
}

/// PUT /api/admin/settings/runtime
/// 设置运行时调度配置
pub async fn set_runtime_settings(
    State(state): State<AdminState>,
    Json(payload): Json<SetRuntimeSettingsRequest>,
) -> impl IntoResponse {
    match state.service.set_runtime_settings(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PATCH /api/admin/credentials/:id/policy
/// 设置凭据调度策略覆盖
pub async fn set_credential_policy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetCredentialPolicyRequest>,
) -> impl IntoResponse {
    match state.service.set_policy(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 调度策略已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/policy/batch
/// 批量设置凭据调度策略覆盖
pub async fn set_credential_policy_batch(
    State(state): State<AdminState>,
    Json(payload): Json<BatchCredentialPolicyRequest>,
) -> impl IntoResponse {
    let count = payload.ids.len();
    match state.service.set_policy_batch(payload) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "已更新 {} 个凭据的调度策略",
            count
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/cooldown/clear
/// 清除单个凭据冷却状态
pub async fn clear_credential_cooldown(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.clear_cooldown(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 冷却状态已清除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/cooldown/clear-batch
/// 批量清除凭据冷却状态
pub async fn clear_credential_cooldown_batch(
    State(state): State<AdminState>,
    Json(payload): Json<BatchCredentialIdsRequest>,
) -> impl IntoResponse {
    let count = payload.ids.len();
    match state.service.clear_cooldown_batch(payload) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "已清除 {} 个凭据的冷却状态",
            count
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/export
/// 批量导出明文凭据
pub async fn export_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<ExportCredentialsRequest>,
) -> impl IntoResponse {
    match state.service.export_credentials(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}
