//! Peer 数据平面 API handler（Epic 4，均需登录的普通用户 CurrentUser）。
//!
//! 端点：
//! - POST   /api/v1/peers/register   注册节点（Story 4.5）
//! - POST   /api/v1/peers/heartbeat  心跳上报（Story 4.6）
//! - DELETE /api/v1/peers/me         注销当前节点（Story 4.7）
//! - GET    /api/v1/peers/me/config  下载客户端配置文件（Story 4.7）

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use vpn_api_types::{
    peer::{PeerHeartbeatRequest, PeerRegisterRequest, PeerRegisterResponse},
    ApiResponse,
};

use crate::{auth::CurrentUser, error::ApiError, state::AppState};

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
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.peer_service()?;
    svc.heartbeat(
        &current.user_id,
        body.endpoint.as_deref(),
        state.clock.now_unix_ms(),
    )
    .await?;
    Ok(success(&state, ()))
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
