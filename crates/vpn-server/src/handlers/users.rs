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
    group::AssignGroupRequest,
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
        .create_user(
            &body.username,
            &body.email,
            body.password.as_deref(),
            body.max_devices,
        )
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
///
/// 支持更新 status（启用/禁用）与 max_devices（终端数量上限），至少携带其一。
#[tracing::instrument(skip(state, body))]
pub async fn update_user(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<ApiResponse<UserDto>>, ApiError> {
    let svc = state.user_service()?;
    if body.status.is_none() && body.max_devices.is_none() {
        return Err(AppError::Config("缺少 status / max_devices 字段".to_string()).into());
    }
    let mut dto = None;
    if let Some(max_devices) = body.max_devices {
        dto = Some(svc.update_max_devices(&id, max_devices).await?);
    }
    if let Some(status) = &body.status {
        dto = Some(svc.update_status(&id, status).await?);
        // 禁用即踢隧道:强制下线其节点(摘除 WG peer + 标记 force_removed,可恢复)。
        // peer_service 未装配(如纯用户管理测试场景)则跳过这一联动副作用。
        if status == "disabled" {
            if let Ok(ps) = state.peer_service() {
                ps.force_remove_by_user(&id).await?;
            }
        }
    }
    Ok(success(&state, dto.expect("至少一个字段已校验")))
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

/// PUT /api/v1/admin/users/:id/groups —— 全量设置用户所属组（空列表=取消所有分组）。
#[tracing::instrument(skip(state, body))]
pub async fn set_user_groups(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<AssignGroupRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.user_group_service()?;
    svc.set_user_groups(&id, &body.group_ids).await?;
    Ok(success(&state, ()))
}

/// Story 3.5：DELETE /api/v1/admin/users/:id
#[tracing::instrument(skip(state, admin))]
pub async fn delete_user(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    // 不能删自己（与 user_service.delete_user 内部校验一致）；前置以免给被拒请求误清节点。
    if admin.user_id == id {
        return Err(AppError::NoAccessReason("无法删除自己的账号".to_string()).into());
    }
    // 必须**先**清节点再删用户行：peers.user_id -> users.id 外键无级联，先删 users 会触发外键
    // 冲突（删任何有 peer 行的用户都会失败）。purge_by_user 摘 WG + 硬删其全部 peer 行 + 回收 IP。
    // peer_service / user_group_service 未装配（纯用户管理测试场景）则跳过对应联动。
    if let Ok(ps) = state.peer_service() {
        ps.purge_by_user(&id).await?;
    }
    let svc = state.user_service()?;
    svc.delete_user(&admin.user_id, &id).await?;
    // 联动清理其组成员关联，避免悬挂行使组 member_count 永久虚高（user_group_members 无 FK 级联）。
    if let Ok(gs) = state.user_group_service() {
        let _ = gs.remove_user_from_groups(&id).await;
    }
    Ok(success(&state, ()))
}
