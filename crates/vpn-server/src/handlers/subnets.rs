//! 网段目录 API handler（全部需 admin 权限）。
//!
//! - GET    /api/v1/admin/subnets        列出
//! - POST   /api/v1/admin/subnets        新增
//! - PATCH  /api/v1/admin/subnets/:id     改名 / 改 CIDR
//! - DELETE /api/v1/admin/subnets/:id     删除

use axum::{
    extract::{Path, State},
    Json,
};
use vpn_api_types::{
    subnet::{CreateSubnetRequest, SubnetDto, UpdateSubnetRequest},
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
pub async fn list_subnets(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<Vec<SubnetDto>>>, ApiError> {
    let svc = state.subnet_service()?;
    Ok(success(&state, svc.list().await?))
}

#[tracing::instrument(skip(state, body))]
pub async fn create_subnet(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<CreateSubnetRequest>,
) -> Result<Json<ApiResponse<SubnetDto>>, ApiError> {
    let svc = state.subnet_service()?;
    Ok(success(&state, svc.create(&body.name, &body.cidr).await?))
}

#[tracing::instrument(skip(state, body))]
pub async fn update_subnet(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateSubnetRequest>,
) -> Result<Json<ApiResponse<SubnetDto>>, ApiError> {
    let svc = state.subnet_service()?;
    let dto = svc
        .update(&id, body.name.as_deref(), body.cidr.as_deref())
        .await?;
    Ok(success(&state, dto))
}

#[tracing::instrument(skip(state))]
pub async fn delete_subnet(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.subnet_service()?;
    svc.delete(&id).await?;
    Ok(success(&state, ()))
}
