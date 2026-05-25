//! 系统信息 handler。

use axum::{extract::State, Json};
use vpn_api_types::{system::SystemInfo, ApiResponse};

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

#[tracing::instrument(skip(state))]
pub async fn system_info(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<SystemInfo>>, ApiError> {
    // Story 2.9 MVP：返回占位值。
    // Story 4.3 引入服务端 WG keypair 后，server_public_key 等字段读取真实值。
    let info = SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        vpn_subnet: "10.8.0.0/24".to_string(),
        server_public_key: "<待 Story 4.3 实现 WireGuard runtime 后填充>".to_string(),
        server_endpoint: "<待配置>".to_string(),
        listen_port: 51820,
        started_at: state.clock.now_unix_ms(),
    };
    Ok(Json(ApiResponse::success(
        info,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    )))
}
