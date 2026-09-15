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
    Ok(success(
        &state,
        svc.create(
            &body.name,
            &resolve_cidrs(Some(body.cidr), body.cidrs)?.unwrap_or_default(),
        )
        .await?,
    ))
}

#[tracing::instrument(skip(state, body))]
pub async fn update_subnet(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateSubnetRequest>,
) -> Result<Json<ApiResponse<SubnetDto>>, ApiError> {
    let svc = state.subnet_service()?;
    let cidr = resolve_cidrs(body.cidr, body.cidrs)?;
    let dto = svc
        .update(&id, body.name.as_deref(), cidr.as_deref())
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

/// 新数组字段与旧文本字段不可同时赋值，避免静默忽略输入。
fn resolve_cidrs(
    legacy: Option<String>,
    cidrs: Option<Vec<String>>,
) -> Result<Option<String>, ApiError> {
    if let Some(cidrs) = cidrs {
        if legacy.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(vpn_core::AppError::Validation("cidr 与 cidrs 不能同时提供".into()).into());
        }
        Ok(Some(cidrs.join(",")))
    } else {
        Ok(legacy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_legacy_and_array_requests_without_silent_conflicts() {
        let legacy: CreateSubnetRequest =
            serde_json::from_str(r#"{"name":"old","cidr":"10.0.0.0/8"}"#).unwrap();
        assert_eq!(
            resolve_cidrs(Some(legacy.cidr), legacy.cidrs)
                .unwrap()
                .as_deref(),
            Some("10.0.0.0/8")
        );
        let group: CreateSubnetRequest =
            serde_json::from_str(r#"{"name":"group","cidrs":["10.0.0.0/8","172.16.0.0/12"]}"#)
                .unwrap();
        assert_eq!(
            resolve_cidrs(Some(group.cidr), group.cidrs)
                .unwrap()
                .as_deref(),
            Some("10.0.0.0/8,172.16.0.0/12")
        );
        assert!(resolve_cidrs(Some("10.0.0.0/8".into()), Some(vec![])).is_err());
        assert_eq!(
            resolve_cidrs(None, Some(vec![])).unwrap(),
            Some(String::new())
        );
        assert_eq!(resolve_cidrs(None, None).unwrap(), None);
    }
}
