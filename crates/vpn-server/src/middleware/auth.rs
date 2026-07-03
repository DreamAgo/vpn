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
