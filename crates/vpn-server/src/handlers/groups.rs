//! 用户组管理 API handler（全部需 admin 权限）。
//!
//! 端点：
//! - GET    /api/v1/admin/groups          列出所有组（含成员数）
//! - POST   /api/v1/admin/groups          创建组
//! - PATCH  /api/v1/admin/groups/:id       改名 / 改可路由网段
//! - DELETE /api/v1/admin/groups/:id       删除组（成员自动解组）

use axum::{
    extract::{Path, State},
    Json,
};
use vpn_api_types::{
    group::{CreateUserGroupRequest, UpdateUserGroupRequest, UserGroupDto},
    ApiResponse,
};

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    ))
}

#[tracing::instrument(skip(state))]
pub async fn list_groups(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<Vec<UserGroupDto>>>, ApiError> {
    let svc = state.user_group_service()?;
    Ok(success(&state, svc.list().await?))
}

#[tracing::instrument(skip(state, body))]
pub async fn create_group(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<CreateUserGroupRequest>,
) -> Result<Json<ApiResponse<UserGroupDto>>, ApiError> {
    let svc = state.user_group_service()?;
    let dto = svc.create(&body.name, &body.routes).await?;
    Ok(success(&state, dto))
}

#[tracing::instrument(skip(state, body))]
pub async fn update_group(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserGroupRequest>,
) -> Result<Json<ApiResponse<UserGroupDto>>, ApiError> {
    let svc = state.user_group_service()?;
    let dto = svc
        .update(&id, body.name.as_deref(), body.routes.as_deref())
        .await?;
    Ok(success(&state, dto))
}

#[tracing::instrument(skip(state))]
pub async fn delete_group(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.user_group_service()?;
    svc.delete(&id).await?;
    Ok(success(&state, ()))
}
