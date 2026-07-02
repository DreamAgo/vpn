//! Peer 数据平面 API handler（Epic 4，均需登录的普通用户 CurrentUser）。
//!
//! 端点：
//! - POST   /api/v1/peers/register   注册节点（Story 4.5）
//! - POST   /api/v1/peers/heartbeat  心跳上报（Story 4.6）
//! - DELETE /api/v1/peers/me         注销当前节点（Story 4.7）
//! - GET    /api/v1/peers/me/config  下载客户端配置文件（Story 4.7）

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use vpn_api_types::{
    peer::{
        AdminPeerQuery, AdminPeerView, PeerHeartbeatRequest, PeerHeartbeatResponse,
        PeerRegisterRequest, PeerRegisterResponse, UpdatePeerRoutesRequest,
    },
    ApiResponse, Page,
};

use crate::{
    auth::{CurrentUser, RequireAdmin},
    error::ApiError,
    state::AppState,
};

fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    ))
}

/// Story 4.5：POST /api/v1/peers/register
#[tracing::instrument(skip(state, body, current))]
pub async fn register(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<PeerRegisterRequest>,
) -> Result<Json<ApiResponse<PeerRegisterResponse>>, ApiError> {
    let svc = state.peer_service()?;
    let resp = svc.register(&current.user_id, &body).await?;
    Ok(success(&state, resp))
}

/// Story 4.6：POST /api/v1/peers/heartbeat
#[tracing::instrument(skip(state, body, current))]
pub async fn heartbeat(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<PeerHeartbeatRequest>,
) -> Result<Json<ApiResponse<PeerHeartbeatResponse>>, ApiError> {
    let svc = state.peer_service()?;
    // Story 5.5：若该 peer 已被 admin 强制下线，心跳被拒（TokenExpired → 401，提示重新登录）。
    let allowed_routes = svc
        .heartbeat_checked(
            &current.user_id,
            body.endpoint.as_deref(),
            state.clock.now_unix_ms(),
        )
        .await?;
    Ok(success(&state, PeerHeartbeatResponse { allowed_routes }))
}

/// Story 4.7：DELETE /api/v1/peers/me
#[tracing::instrument(skip(state, current))]
pub async fn delete_me(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.peer_service()?;
    svc.delete_me(&current.user_id).await?;
    Ok(success(&state, ()))
}

/// Story 4.7：GET /api/v1/peers/me/config（文件下载，非 ApiResponse 信封）。
#[tracing::instrument(skip(state, current))]
pub async fn download_config(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    let svc = state.peer_service()?;
    let dl = svc.render_config(&current.user_id).await?;
    let response = (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", dl.filename),
            ),
        ],
        dl.content,
    )
        .into_response();
    Ok(response)
}

/// Story 5.5：GET /api/v1/admin/peers（需 admin）
#[tracing::instrument(skip(state))]
pub async fn list_admin_peers(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Query(query): Query<AdminPeerQuery>,
) -> Result<Json<ApiResponse<Page<AdminPeerView>>>, ApiError> {
    let svc = state.peer_service()?;
    let page = svc.list_admin_peers(&query).await?;
    Ok(success(&state, page))
}

/// PATCH /api/v1/admin/peers/:id（需 admin，编辑路由网段 / 异地组网）
#[tracing::instrument(skip(state, body))]
pub async fn update_peer_routes(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdatePeerRoutesRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.peer_service()?;
    svc.update_peer_routes(&id, &body.routed_subnets).await?;
    Ok(success(&state, ()))
}

/// Story 5.5：DELETE /api/v1/admin/peers/:id（需 admin，强制下线）
#[tracing::instrument(skip(state))]
pub async fn force_remove_peer(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.peer_service()?;
    svc.force_remove(&id).await?;
    Ok(success(&state, ()))
}

/// DELETE /api/v1/admin/peers/:id/purge（需 admin，彻底删除节点 + 回收 VPN IP）
#[tracing::instrument(skip(state))]
pub async fn purge_peer(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.peer_service()?;
    svc.purge(&id).await?;
    Ok(success(&state, ()))
}
