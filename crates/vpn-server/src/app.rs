//! Axum Router 工厂。

use axum::{routing::get, Router};
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::{handlers, state::AppState};

const REQUEST_ID_HEADER: &str = "x-request-id";

/// 构造完整的 Axum Router（含中间件链）。
///
/// 中间件执行顺序（请求路径）：
/// 1. SetRequestIdLayer 注入 X-Request-Id（若客户端未带则生成 UUID v4）
/// 2. TraceLayer 记录结构化日志（请求/响应/耗时）
/// 3. handler 业务逻辑
/// 4. PropagateRequestIdLayer 把 request_id 复制到响应头
pub fn build_router(state: AppState) -> Router {
    let request_id_header = axum::http::HeaderName::from_static(REQUEST_ID_HEADER);

    let middleware = ServiceBuilder::new()
        .layer(SetRequestIdLayer::new(
            request_id_header.clone(),
            MakeRequestUuid,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::new(request_id_header));

    Router::new()
        .route("/health", get(handlers::health::health_handler))
        .with_state(state)
        .layer(middleware)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_endpoint_returns_200() {
        let app = build_router(AppState::new());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn request_id_header_propagated() {
        let app = build_router(AppState::new());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert!(response.headers().contains_key("x-request-id"));
    }
}
