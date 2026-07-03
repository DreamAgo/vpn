//! Service account API key handlers.

use axum::{
    extract::{Path, State},
    Json,
};
use vpn_api_types::{
    api_key::{ApiKeyDto, CreateApiKeyRequest, CreateApiKeyResponse},
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

pub async fn create_api_key(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<ApiResponse<CreateApiKeyResponse>>, ApiError> {
    let svc = state.api_key_service()?;
    let resp = svc.create(&body.name, &body.scopes, &admin.user_id).await?;
    Ok(success(&state, resp))
}

pub async fn list_api_keys(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<Vec<ApiKeyDto>>>, ApiError> {
    let svc = state.api_key_service()?;
    Ok(success(&state, svc.list().await?))
}

pub async fn revoke_api_key(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.api_key_service()?;
    svc.revoke(&id).await?;
    Ok(success(&state, ()))
}
