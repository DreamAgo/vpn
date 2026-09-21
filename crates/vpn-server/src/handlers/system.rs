//! 系统信息 handler。

use axum::extract::{rejection::JsonRejection, Query};
use axum::{extract::State, Json};
use vpn_api_types::{
    system::{
        EmailNotificationSettings, FeishuApprovalSubscriptionView, IntegrationSettingsView,
        NetworkSettingsView, NotificationEventQuery, NotificationEventView, SystemInfo,
        TestEmailNotificationRequest, UpdateEmailNotificationSettingsRequest,
        UpdateIntegrationSettingsRequest, UpdateNetworkSettingsRequest, UpdateServerRoutesRequest,
    },
    ApiResponse,
};
use vpn_core::AppError;

use crate::{
    auth::RequireAdmin, error::ApiError, services::peer_service::route_policy_lock, state::AppState,
};

#[tracing::instrument(skip(state))]
pub async fn system_info(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<SystemInfo>>, ApiError> {
    let svc = state.peer_service()?;
    let endpoint = svc.public_data_endpoint().to_string();
    // listen_port：从 endpoint(host:port) 解析，失败回退默认 51820。
    let listen_port = endpoint
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(51820);
    let info = SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        vpn_subnet: svc.vpn_subnet_cidr(),
        server_public_key: svc.server_public_key_str().to_string(),
        server_endpoint: endpoint,
        listen_port,
        started_at: state.clock.now_unix_ms(),
        server_routes: svc.server_routes().await,
    };
    Ok(Json(ApiResponse::success(
        info,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// GET /api/v1/admin/integrations/settings：读取脱敏的 applied/desired 集成配置。
#[tracing::instrument(skip(state))]
pub async fn integration_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<IntegrationSettingsView>>, ApiError> {
    let settings = state.integration_settings_service()?.view().await?;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// 读取当前已应用身份的本地订阅成功记录。
#[tracing::instrument(skip(state))]
pub async fn approval_subscription(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<FeishuApprovalSubscriptionView>>, ApiError> {
    let status = state
        .integration_settings_service()?
        .approval_subscription()
        .await?;
    Ok(Json(ApiResponse::success(
        status,
        "n/a".into(),
        state.clock.now_unix_ms(),
    )))
}

#[tracing::instrument(skip(state))]
pub async fn subscribe_approval(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<FeishuApprovalSubscriptionView>>, ApiError> {
    let status = state
        .integration_settings_service()?
        .subscribe_approval()
        .await?;
    Ok(Json(ApiResponse::success(
        status,
        "n/a".into(),
        state.clock.now_unix_ms(),
    )))
}

/// PUT /api/v1/admin/integrations/settings：原子保存 desired，重启后生效。
#[tracing::instrument(skip(state, body))]
pub async fn update_integration_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    body: Result<Json<UpdateIntegrationSettingsRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<IntegrationSettingsView>>, ApiError> {
    let Json(body) =
        body.map_err(|error| AppError::Validation(format!("集成设置请求格式非法：{error}")))?;
    let backend = state
        .network_settings_service()?
        .applied()
        .vpn
        .wg_backend
        .clone();
    let settings = state
        .integration_settings_service()?
        .update(body, &backend)
        .await?;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// GET /api/v1/admin/network/settings：读取网络参数（需 admin）。
#[tracing::instrument(skip(state))]
pub async fn network_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<NetworkSettingsView>>, ApiError> {
    let lock = route_policy_lock();
    let _guard = lock.lock().await;
    let service = state.network_settings_service()?;
    let settings = service.view(service.server_routes().await?).await;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// PUT /api/v1/admin/network/settings：原子更新网络参数（需 admin）。
#[tracing::instrument(skip(state, body))]
pub async fn update_network_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    body: Result<Json<UpdateNetworkSettingsRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<NetworkSettingsView>>, ApiError> {
    let Json(body) =
        body.map_err(|error| AppError::Validation(format!("网络参数请求格式非法：{error}")))?;
    let service = state.network_settings_service()?;
    let lock = route_policy_lock();
    let _guard = lock.lock().await;
    let routes = service
        .update_locked(
            body.desired,
            &body.server_routes,
            body.local_route_bypass.as_deref(),
        )
        .await?;
    if let Some(peer_service) = &state.peer_service {
        peer_service.apply_server_routes(routes.clone()).await;
    }
    state.refresh_network_acl().await?;
    let settings = service.view(routes).await;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// PUT /api/v1/admin/system/routes：更新服务端 LAN 网段（需 admin）。
///
/// 返回规整后的网段列表。变更对新接入/重连的客户端立即生效。
#[tracing::instrument(skip(state, body))]
pub async fn update_server_routes(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<UpdateServerRoutesRequest>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let svc = state.peer_service()?;
    let routes = svc.set_server_routes(&body.routes).await?;
    state.refresh_network_acl().await?;
    Ok(Json(ApiResponse::success(
        routes,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// GET /api/v1/admin/notifications/email：读取邮件通知配置（需 admin）。
#[tracing::instrument(skip(state))]
pub async fn email_notification_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<EmailNotificationSettings>>, ApiError> {
    let svc = state.notification_service()?;
    let settings = svc.email_settings().await?;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// PUT /api/v1/admin/notifications/email：更新邮件通知配置（需 admin）。
#[tracing::instrument(skip(state, body))]
pub async fn update_email_notification_settings(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<UpdateEmailNotificationSettingsRequest>,
) -> Result<Json<ApiResponse<EmailNotificationSettings>>, ApiError> {
    let svc = state.notification_service()?;
    let settings = svc.update_email_settings(body).await?;
    Ok(Json(ApiResponse::success(
        settings,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// POST /api/v1/admin/notifications/email/test：发送测试邮件（需 admin）。
#[tracing::instrument(skip(state, body))]
pub async fn test_email_notification(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<TestEmailNotificationRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.notification_service()?;
    svc.send_test_email(body).await?;
    Ok(Json(ApiResponse::success(
        (),
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// GET /api/v1/admin/notifications/events：通知历史（需 admin）。
#[tracing::instrument(skip(state))]
pub async fn list_notification_events(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Query(query): Query<NotificationEventQuery>,
) -> Result<Json<ApiResponse<Vec<NotificationEventView>>>, ApiError> {
    let svc = state.notification_service()?;
    let events = svc.list_events(&query).await?;
    Ok(Json(ApiResponse::success(
        events,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}

/// POST /api/v1/admin/system/restart：管理员请求重启，先返回响应再关闭监听。
pub async fn restart_server(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let tx = state
        .restart_tx
        .as_ref()
        .ok_or_else(|| AppError::Config("当前运行环境不支持在线重启".to_string()))?;
    let pool = state.db_pool()?;
    let mut audit_tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    crate::middleware::audit_context::record(
        &mut audit_tx,
        "system/restart-request",
        serde_json::Value::Null,
        serde_json::json!({"restart_requested":true}),
    )
    .await?;
    audit_tx
        .commit()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    crate::middleware::audit_context::committed();
    tx.send(true)
        .map_err(|_| AppError::Config("服务正在关闭，请稍后检查运行状态".to_string()))?;
    tracing::info!("管理员请求重启服务端");
    Ok(Json(ApiResponse::success(
        (),
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}
