//! CurrentUser + RequireAdmin axum extractor。
//!
//! Story 2.7 实现 AuthLayer middleware 后，extractor 从 request extension 读取 CurrentUser。

use axum::{extract::FromRequestParts, http::request::Parts};
use vpn_core::AppError;

use crate::error::ApiError;

/// 当前已认证用户（由 AuthLayer 注入到 request extension）。
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user_id: String,
    pub role: String,
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| ApiError::from(AppError::MissingAuth))
    }
}

/// 校验当前用户是 admin。
///
/// 用法：handler 函数签名加 `RequireAdmin(_)` 参数。
#[derive(Debug)]
pub struct RequireAdmin(pub CurrentUser);

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let current = CurrentUser::from_request_parts(parts, state).await?;
        if current.role == "admin" {
            Ok(RequireAdmin(current))
        } else {
            Err(ApiError::from(AppError::RequireAdmin))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[tokio::test]
    async fn extractor_returns_missing_auth_when_no_user_in_extension() {
        let req: Request<()> = Request::builder().body(()).unwrap();
        let (mut parts, _) = req.into_parts();
        let result = CurrentUser::from_request_parts(&mut parts, &()).await;
        let err = result.unwrap_err();
        assert!(matches!(err.inner, AppError::MissingAuth));
    }

    #[tokio::test]
    async fn extractor_returns_user_when_present_in_extension() {
        let mut req: Request<()> = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(CurrentUser {
            user_id: "user-1".to_string(),
            role: "admin".to_string(),
        });
        let (mut parts, _) = req.into_parts();
        let user = CurrentUser::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(user.user_id, "user-1");
        assert_eq!(user.role, "admin");
    }

    #[tokio::test]
    async fn require_admin_rejects_non_admin_user() {
        let mut req: Request<()> = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(CurrentUser {
            user_id: "user-1".to_string(),
            role: "user".to_string(),
        });
        let (mut parts, _) = req.into_parts();
        let result = RequireAdmin::from_request_parts(&mut parts, &()).await;
        let err = result.unwrap_err();
        assert!(matches!(err.inner, AppError::RequireAdmin));
    }
}
