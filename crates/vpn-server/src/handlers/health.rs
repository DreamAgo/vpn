//! 健康检查端点。

use axum::{extract::State, Json};
use serde::Serialize;

use crate::state::AppState;
use vpn_api_types::ApiResponse;

#[derive(Serialize)]
pub struct HealthData {
    pub status: &'static str,
    pub version: &'static str,
}

/// GET /health — 健康检查（无认证）。
///
/// 用途：
/// - Docker / k8s healthcheck
/// - 反向代理 upstream 检测
/// - 负载均衡器探活
#[tracing::instrument(skip(state))]
pub async fn health_handler(State(state): State<AppState>) -> Json<ApiResponse<HealthData>> {
    let data = HealthData {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    };
    // 这里 request_id 暂用占位（middleware 注入在 Story 1.5 由 tower-http 处理）
    Json(ApiResponse::success(
        data,
        "health".to_string(),
        state.clock.now_unix_ms(),
    ))
}
