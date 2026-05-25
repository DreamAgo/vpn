//! 用户管理 API handler（Epic 3，全部需 admin 权限）。
//!
//! 端点：
//! - POST   /api/v1/admin/users              创建用户
//! - GET    /api/v1/admin/users              列表 + 搜索 + 分页
//! - PATCH  /api/v1/admin/users/:id          启用 / 禁用
//! - POST   /api/v1/admin/users/:id/reset-password  重置密码
//! - DELETE /api/v1/admin/users/:id          删除（级联）

use axum::{
    extract::{Path, Query, State},
    Json,
};
use vpn_api_types::{
    user::{
        CreateUserRequest, CreateUserResponse, ListUsersQuery, ResetPasswordResponse,
        UpdateUserRequest, UserDto,
    },
    ApiResponse, Page,
};
use vpn_core::AppError;

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    ))
}

/// Story 3.1：POST /api/v1/admin/users
#[tracing::instrument(skip(state, body))]
pub async fn create_user(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<CreateUserRequest>,
) -> Result<Json<ApiResponse<CreateUserResponse>>, ApiError> {
    let svc = state.user_service()?;
    let resp = svc
        .create_user(&body.username, &body.email, body.password.as_deref())
        .await?;
    Ok(success(&state, resp))
}

/// Story 3.2：GET /api/v1/admin/users
#[tracing::instrument(skip(state))]
pub async fn list_users(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<ApiResponse<Page<UserDto>>>, ApiError> {
    let svc = state.user_service()?;
    let page = svc.list_users(&query).await?;
    Ok(success(&state, page))
}

/// Story 3.3：PATCH /api/v1/admin/users/:id
#[tracing::instrument(skip(state, body))]
pub async fn update_user(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<ApiResponse<UserDto>>, ApiError> {
    let svc = state.user_service()?;
    let status = body
        .status
        .ok_or_else(|| AppError::Config("缺少 status 字段".to_string()))?;
    let dto = svc.update_status(&id, &status).await?;
    Ok(success(&state, dto))
}

/// Story 3.4：POST /api/v1/admin/users/:id/reset-password
#[tracing::instrument(skip(state, admin))]
pub async fn reset_password(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ResetPasswordResponse>>, ApiError> {
    let svc = state.user_service()?;
    let new_password = svc.reset_password(&admin.user_id, &id).await?;
    Ok(success(&state, ResetPasswordResponse { new_password }))
}

/// Story 3.5：DELETE /api/v1/admin/users/:id
#[tracing::instrument(skip(state, admin))]
pub async fn delete_user(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.user_service()?;
    svc.delete_user(&admin.user_id, &id).await?;
    Ok(success(&state, ()))
}
