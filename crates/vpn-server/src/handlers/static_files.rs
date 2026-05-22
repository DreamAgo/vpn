//! 嵌入式前端静态文件 + SPA fallback。
//!
//! 通过 rust-embed 把 `frontend/dist/` 编译进二进制。
//! 非 `/api/*` 和 `/ws/*` 的 GET 请求都走此 handler。
//!
//! Fallback 策略：
//! 1. 请求路径匹配某个文件 → 返回该文件（含正确 Content-Type）
//! 2. 否则返回 `index.html`（让 React Router 处理路由）

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../frontend/dist/"]
struct FrontendAssets;

pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    // 去除前导 /
    let path = uri.path().trim_start_matches('/');

    // 尝试直接匹配文件
    if let Some(content) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(content.data.into_owned()))
            .unwrap();
    }

    // SPA fallback: 返回 index.html
    if let Some(content) = FrontendAssets::get("index.html") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(content.data.into_owned()))
            .unwrap();
    }

    // 极端兜底：前端未构建
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from(
            "前端资源未嵌入。请先在 frontend/ 目录运行 `npm run build`，然后重新编译 vpn-server。",
        ))
        .unwrap()
}
