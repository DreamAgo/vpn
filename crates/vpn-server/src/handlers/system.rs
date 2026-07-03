//! 系统信息 handler。

use axum::extract::Query;
use axum::{extract::State, Json};
use vpn_api_types::{
    system::{
        EmailNotificationSettings, NotificationEventQuery, NotificationEventView, SystemInfo,
        TestEmailNotificationRequest, UpdateEmailNotificationSettingsRequest,
        UpdateServerRoutesRequest,
    },
    ApiResponse,
};

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

#[tracing::instrument(skip(state))]
pub async fn system_info(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<SystemInfo>>, ApiError> {
    let svc = state.peer_service()?;
    let endpoint = svc.server_endpoint().to_string();
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
