use crate::{
    auth::RequireAdmin,
    error::ApiError,
    services::{ApprovalEventHeaders, ApprovalWebhookReply},
    AppState,
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use vpn_api_types::{
    user::{FeishuBindingDto, FeishuLookupRequest},
    ApiResponse,
};
fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".into(),
        state.clock.now_unix_ms(),
    ))
}
pub async fn lookup(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(req): Json<FeishuLookupRequest>,
) -> Result<Json<ApiResponse<FeishuBindingDto>>, ApiError> {
    Ok(success(
        &state,
        state.feishu_directory_service()?.lookup(&req).await?,
    ))
}
pub async fn bind(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(req): Json<FeishuLookupRequest>,
) -> Result<Json<ApiResponse<Vec<FeishuBindingDto>>>, ApiError> {
    Ok(success(
        &state,
        state.feishu_directory_service()?.bind(&id, &req).await?,
    ))
}
pub async fn sync_user(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<FeishuBindingDto>>>, ApiError> {
    Ok(success(
        &state,
        state.feishu_directory_service()?.sync_user(&id).await?,
    ))
}
pub async fn sync_all(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<u64>>, ApiError> {
    Ok(success(
        &state,
        state.feishu_directory_service()?.queue_all().await?,
    ))
}
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let header = |key: &str| {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
    };
    let reply = state
        .feishu_directory_service()?
        .receive(
            ApprovalEventHeaders {
                timestamp: header("x-lark-request-timestamp"),
                nonce: header("x-lark-request-nonce"),
                signature: header("x-lark-signature"),
            },
            &body,
        )
        .await?;
    Ok(Json(match reply {
        ApprovalWebhookReply::Ack => serde_json::json!({"code":0}),
        ApprovalWebhookReply::Challenge(value) => serde_json::json!({"challenge":value}),
    }))
}
