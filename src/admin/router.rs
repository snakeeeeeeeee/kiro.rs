//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, patch, post},
};

use super::{
    handlers::{
        add_credential, clear_credential_cooldown, clear_credential_cooldown_batch,
        delete_credential, export_credentials, force_refresh_token, get_all_credentials,
        get_credential_balance, get_load_balancing_mode, get_runtime_settings, get_runtime_status,
        reset_failure_count, set_credential_disabled, set_credential_policy,
        set_credential_policy_batch, set_credential_priority, set_load_balancing_mode,
        set_runtime_settings,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `POST /credentials/export` - 批量导出明文凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
/// - `GET /runtime` - 获取运行时状态
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/export", post(export_credentials))
        .route(
            "/credentials/policy/batch",
            post(set_credential_policy_batch),
        )
        .route(
            "/credentials/cooldown/clear-batch",
            post(clear_credential_cooldown_batch),
        )
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/policy", patch(set_credential_policy))
        .route(
            "/credentials/{id}/cooldown/clear",
            post(clear_credential_cooldown),
        )
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/runtime", get(get_runtime_status))
        .route(
            "/settings/runtime",
            get(get_runtime_settings).put(set_runtime_settings),
        )
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
