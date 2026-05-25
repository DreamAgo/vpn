//! AppError -> Axum HTTP 响应转换。
//!
//! 设计原则：
//! - 所有 handler 返回 `Result<impl IntoResponse, AppError>`
//! - 客户端错误（4xxx）→ HTTP 400；服务端错误（5xxx）→ HTTP 500
//! - 业务错误码与人类可读消息走 ApiResponse 信封返回
//! - 服务端错误的内部细节通过 tracing 记录，不暴露给客户端

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use vpn_api_types::ApiResponse;
use vpn_core::AppError;

/// Axum 用的错误包装。包含 AppError + 请求上下文（request_id, timestamp）。
#[derive(Debug)]
pub struct ApiError {
    pub inner: AppError,
    pub request_id: String,
    pub timestamp_ms: i64,
}

impl ApiError {
    pub fn new(err: AppError, request_id: String, timestamp_ms: i64) -> Self {
        Self {
            inner: err,
            request_id,
            timestamp_ms,
        }
    }
}

/// 从 AppError 自动转 ApiError（用于 extractor 拒绝、handler `?` 运算符）。
///
/// request_id 与 timestamp 使用默认值；正常 handler 错误路径应通过
/// middleware 注入正确的 request_id（Story 2.7 + 优化），此处的兜底
/// 保证类型转换可用。
impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        Self {
            inner: err,
            request_id: "n/a".to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = if self.inner.is_client_error() {
            // 认证错误用 401，权限用 403，资源用 404，限速用 429，其他客户端用 400
            match self.inner.code() {
                1001..=1099 => StatusCode::UNAUTHORIZED,
                2001..=2099 => StatusCode::FORBIDDEN,
                3001..=3099 => StatusCode::NOT_FOUND,
                4001..=4099 => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::BAD_REQUEST,
            }
        } else {
            // 服务端错误：记录详细日志但只返回通用消息
            tracing::error!(
                error = ?self.inner,
                request_id = %self.request_id,
                "Internal server error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        };

        let body: ApiResponse<()> = ApiResponse::error(
            self.inner.code(),
            self.inner.to_string(),
            self.request_id,
            self.timestamp_ms,
        );

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_errors_map_to_correct_http_status() {
        let err = ApiError::new(AppError::InvalidCredentials, "r1".to_string(), 100);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let err = ApiError::new(AppError::RequireAdmin, "r2".to_string(), 100);
        assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);

        let err = ApiError::new(AppError::UserNotFound, "r3".to_string(), 100);
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);

        let err = ApiError::new(AppError::RateLimited, "r4".to_string(), 100);
        assert_eq!(err.into_response().status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn server_errors_return_500() {
        let err = ApiError::new(
            AppError::WireGuard("test".to_string()),
            "r5".to_string(),
            100,
        );
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
