//! AuthLayer：从 Authorization Bearer 解析 JWT，注入 CurrentUser 到 request extension。
//!
//! Story 2.7。

use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, HeaderName},
    middleware::Next,
    response::Response,
};
use vpn_core::AppError;

use crate::{auth::CurrentUser, error::ApiError, state::AppState};

const API_KEY_HEADER: HeaderName = HeaderName::from_static("x-api-key");

/// 必须认证的中间件。
///
/// 用法：`.layer(axum::middleware::from_fn_with_state(state.clone(), require_auth))`
pub async fn require_auth(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = request.uri().path().to_string();
    let ip = super::audit::client_ip(
        request.headers(),
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|p| p.0),
        &state.trusted_proxies,
    );
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(128).collect::<String>());
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(512).collect::<String>());
    let result = authenticate(axum::extract::State(state.clone()), request, next).await;
    if let (Err(error), Some(audit)) = (&result, &state.audit_service) {
        audit
            .log(
                crate::repositories::AuditLogEntry {
                    action: "auth.rejected".into(),
                    resource: path,
                    user_agent,
                    ip_addr: ip,
                    status_code: Some(crate::error::status_code(&error.inner).as_u16() as i32),
                    metadata: Some(
                        serde_json::json!({"outcome":"failed","reason_code":error.inner.code(),"request_id":request_id})
                            .to_string(),
                    ),
                    ..Default::default()
                },
                state.clock.now_unix_ms(),
            )
            .await;
    }
    result
}

async fn authenticate(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    if let Some(token) = bearer {
        if token.starts_with("ylk_") {
            if let Some(api_key) = state.api_key_service()?.verify(token).await? {
                request.extensions_mut().insert(CurrentUser {
                    user_id: format!("api_key:{}", api_key.id),
                    role: "admin".to_string(),
                });
                return Ok(next.run(request).await);
            }
            return Err(ApiError::from(AppError::TokenExpired));
        }

        let svc = state.auth_service()?;
        let (user_id, role) = svc.issuer.verify_access(token).await?;
        svc.user_repo.ensure_available(&user_id).await?;
        request
            .extensions_mut()
            .insert(CurrentUser { user_id, role });
        return Ok(next.run(request).await);
    }

    if let Some(key) = request
        .headers()
        .get(API_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(api_key) = state.api_key_service()?.verify(key).await? {
            request.extensions_mut().insert(CurrentUser {
                user_id: format!("api_key:{}", api_key.id),
                role: "admin".to_string(),
            });
            return Ok(next.run(request).await);
        }
        return Err(ApiError::from(AppError::TokenExpired));
    }

    Err(ApiError::from(AppError::MissingAuth))
}
