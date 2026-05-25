//! 审计日志查询 API handler（Story 5.4，需 admin 权限）。
//!
//! 端点：
//! - GET /api/v1/admin/audit-logs   查询审计日志（时间窗 + 过滤 + 分页）

use axum::{
    extract::{Query, State},
    Json,
};
use vpn_api_types::{audit::AuditLogDto, audit::AuditLogQuery, ApiResponse, Page};

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

/// Story 5.4：GET /api/v1/admin/audit-logs
#[tracing::instrument(skip(state))]
pub async fn list_audit_logs(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<ApiResponse<Page<AuditLogDto>>>, ApiError> {
    let svc = state.audit_service()?;
    let page = svc.query(&query, state.clock.now_unix_ms()).await?;
    Ok(Json(ApiResponse::success(
        page,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}
