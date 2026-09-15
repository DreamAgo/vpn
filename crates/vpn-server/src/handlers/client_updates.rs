use crate::{
    auth::RequireAdmin,
    error::ApiError,
    services::client_update_service::{ClientUpdateService, UpdateStatus},
    state::AppState,
};
use axum::{extract::State, Extension, Json};
use serde::Deserialize;
use std::sync::Arc;
use vpn_api_types::ApiResponse;
use vpn_core::AppError;

pub async fn status(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Extension(service): Extension<Arc<ClientUpdateService>>,
) -> Json<ApiResponse<UpdateStatus>> {
    Json(ApiResponse::success(
        service.status().await,
        "n/a".into(),
        state.clock.now_unix_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    auto_sync: bool,
    public_base_url: String,
    #[serde(default)]
    proxy_url: Option<String>,
}
pub async fn configure(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Extension(service): Extension<Arc<ClientUpdateService>>,
    Json(settings): Json<Settings>,
) -> Result<Json<ApiResponse<UpdateStatus>>, ApiError> {
    service
        .configure(
            settings.auto_sync,
            &settings.public_base_url,
            settings.proxy_url.as_deref(),
        )
        .await
        .map_err(AppError::Validation)?;
    Ok(Json(ApiResponse::success(
        service.status().await,
        "n/a".into(),
        state.clock.now_unix_ms(),
    )))
}
pub async fn sync(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Extension(service): Extension<Arc<ClientUpdateService>>,
) -> Result<Json<ApiResponse<UpdateStatus>>, ApiError> {
    service.queue().map_err(AppError::Validation)?;
    Ok(Json(ApiResponse::success(
        service.status().await,
        "n/a".into(),
        state.clock.now_unix_ms(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::CurrentUser;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn client_update_routes_require_admin_and_persist_settings() {
        let directory = tempfile::tempdir().unwrap();
        let service = Arc::new(ClientUpdateService::new(directory.path().to_owned()));
        let router = Router::new()
            .route("/versions", get(status).put(configure))
            .route("/versions/sync", axum::routing::post(sync))
            .layer(Extension(service.clone()))
            .with_state(AppState::new());
        for (method, path, body) in [
            ("GET", "/versions", ""),
            (
                "PUT",
                "/versions",
                r#"{"auto_sync":true,"public_base_url":"https://vpn.example"}"#,
            ),
            ("POST", "/versions/sync", ""),
        ] {
            for role in [None, Some("user")] {
                let mut request = Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap();
                if let Some(role) = role {
                    request.extensions_mut().insert(CurrentUser {
                        user_id: "test".into(),
                        role: role.into(),
                    });
                }
                let response = router.clone().oneshot(request).await.unwrap();
                assert_eq!(
                    response.status(),
                    if role.is_none() {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::FORBIDDEN
                    }
                );
            }
        }
        let mut request = Request::builder()
            .method("PUT")
            .uri("/versions")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"auto_sync":true,"public_base_url":"https://vpn.example","proxy_url":"http://127.0.0.1:7897"}"#,
            ))
            .unwrap();
        request.extensions_mut().insert(CurrentUser {
            user_id: "admin".into(),
            role: "admin".into(),
        });
        assert_eq!(
            router.oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        assert!(service.status().await.record.auto_sync);
        assert_eq!(
            service.status().await.record.proxy_url,
            "http://127.0.0.1:7897/"
        );
    }
}
