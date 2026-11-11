use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use vpn_api_types::{peer::PeerRegisterResponse, ApiResponse};
use vpn_core::AppError;

use crate::{
    auth::{CurrentUser, RequireAdmin},
    cluster::{Candidates, Cluster, Device, DeviceUpdate, Gateway, SyncRequest, SyncResponse},
    error::ApiError,
    services::client_update_service::ClientUpdateService,
    state::AppState,
};

fn cluster(state: &AppState) -> Result<&Cluster, ApiError> {
    state
        .cluster
        .as_deref()
        .ok_or_else(|| AppError::ResourceNotFound("网关集群".into()).into())
}

fn success<T: Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".into(),
        state.clock.now_unix_ms(),
    ))
}

pub(crate) fn device_key(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-vpn-device-key")
        .and_then(|header| header.to_str().ok())
}

#[derive(Deserialize)]
pub struct CandidateRequest {
    wg_public_key: String,
}

#[derive(Deserialize)]
pub struct SelectRequest {
    wg_public_key: String,
    gateway_id: String,
    revision: i64,
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    #[serde(flatten)]
    select: SelectRequest,
    client_version: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
pub struct DefaultRequest {
    gateway_id: String,
}

pub async fn candidates(
    State(state): State<AppState>,
    current: CurrentUser,
    headers: HeaderMap,
    Json(body): Json<CandidateRequest>,
) -> Result<Json<ApiResponse<Candidates>>, ApiError> {
    let result = cluster(&state)?
        .candidates(&current.user_id, &body.wg_public_key, device_key(&headers))
        .await?;
    Ok(success(&state, result))
}

pub async fn connect(
    State(state): State<AppState>,
    current: CurrentUser,
    Extension(updates): Extension<Arc<ClientUpdateService>>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<ApiResponse<PeerRegisterResponse>>, ApiError> {
    let cluster = cluster(&state)?;
    let peer_service = state.peer_service()?;
    let peer = peer_service
        .peer_repo
        .find_active_by_user_and_pubkey(&current.user_id, &body.select.wg_public_key)
        .await?
        .ok_or(AppError::PeerNotFound)?;
    updates
        .enforce_client_version(
            body.client_version
                .as_deref()
                .or(peer.client_version.as_deref()),
        )
        .await
        .map_err(AppError::NoAccessReason)?;
    let result = cluster
        .connect(
            &current.user_id,
            &body.select.wg_public_key,
            device_key(&headers),
            &body.select.gateway_id,
            body.select.revision,
            &body.capabilities,
        )
        .await?;
    if let Some(version) = body.client_version {
        peer_service
            .peer_repo
            .update_client_version(&peer.id, &peer.wg_public_key, &version)
            .await?;
    }
    state.refresh_network_acl().await?;
    Ok(success(&state, result))
}

pub async fn confirm(
    State(state): State<AppState>,
    current: CurrentUser,
    headers: HeaderMap,
    Json(body): Json<SelectRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    cluster(&state)?
        .confirm(
            &current.user_id,
            &body.wg_public_key,
            device_key(&headers),
            &body.gateway_id,
            body.revision,
        )
        .await?;
    Ok(success(&state, ()))
}

pub async fn gateways(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Json<ApiResponse<Vec<Gateway>>>, ApiError> {
    Ok(success(&state, cluster(&state)?.gateways().await?))
}

pub async fn set_default(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(body): Json<DefaultRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    cluster(&state)?.set_default(&body.gateway_id).await?;
    Ok(success(&state, ()))
}

pub async fn device(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Device>>, ApiError> {
    Ok(success(&state, cluster(&state)?.device(&id).await?))
}

pub async fn update_device(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<DeviceUpdate>,
) -> Result<Json<ApiResponse<Device>>, ApiError> {
    Ok(success(
        &state,
        cluster(&state)?.update_device(&id, body).await?,
    ))
}

pub async fn sync(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, ApiError> {
    let service = cluster(&state)?;
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AppError::MissingAuth)?;
    Ok(Json(service.sync(&id, bearer, body).await?))
}
