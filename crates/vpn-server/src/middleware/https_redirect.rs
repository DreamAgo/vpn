//! HTTPS 强制中间件：处理 80 端口的 HTTP 请求，重定向到 HTTPS。
//!
//! 注：在生产环境监听 80 端口仅为：
//! 1. ACME challenge 路径 `/.well-known/acme-challenge/*` 由 rustls-acme 处理
//! 2. 其他所有路径 301 重定向到对应 HTTPS URL

use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::Redirect,
    routing::any,
    Router,
};

/// 构造一个最小的 HTTP-only Router，仅做 HTTPS 重定向。
///
/// 由 main.rs 在 80 端口监听此 router。
pub fn http_to_https_router() -> Router {
    Router::new().fallback(any(redirect_to_https))
}

async fn redirect_to_https(
    headers: HeaderMap,
    uri: Uri,
) -> Result<Redirect, (StatusCode, &'static str)> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::BAD_REQUEST, "missing Host header"))?;

    // 去除可能的 :80 端口
    let host = host.split(':').next().unwrap_or(host);

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("");
    let target = format!("https://{}{}", host, path_and_query);

    Ok(Redirect::permanent(&target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn http_request_redirects_to_https() {
        let app = http_to_https_router();
        let req = Request::builder()
            .uri("/some/path?q=1")
            .header("host", "vpn.example.com")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);

        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "https://vpn.example.com/some/path?q=1");
    }

    #[tokio::test]
    async fn missing_host_returns_400() {
        let app = http_to_https_router();
        let req = Request::builder().uri("/path").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
