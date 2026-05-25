//! AuthLayer：从 Authorization Bearer 解析 JWT，注入 CurrentUser 到 request extension。
//!
//! Story 2.7。

use std::sync::Arc;

use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};
use vpn_core::{service::TokenIssuer, AppError};

use crate::{auth::CurrentUser, error::ApiError};

/// 必须认证的中间件。
///
/// 用法：`.layer(axum::middleware::from_fn_with_state(state.clone(), require_auth))`
pub async fn require_auth(
    axum::extract::State(issuer): axum::extract::State<Arc<dyn TokenIssuer>>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::from(AppError::MissingAuth))?;

    let (user_id, role) = issuer.verify_access(token).await?;
    request
        .extensions_mut()
        .insert(CurrentUser { user_id, role });
    Ok(next.run(request).await)
}
