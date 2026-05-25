//! Epic 2 端到端集成测试：first-time-setup → login → 受保护接口 → change-password。
//!
//! 通过 tower::ServiceExt::oneshot 直接驱动 Router，不绑定真实端口。

use std::str::FromStr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tower::ServiceExt;

use vpn_server::{
    build_router,
    ratelimit::LoginAttempts,
    repositories::{SqliteSessionRepository, SqliteUserRepository},
    services::{Argon2Hasher, AuthService, JwtTokenIssuer},
    AppState,
};

/// 构造带真实 AuthService（内存 SQLite + 临时 RSA 密钥）的 Router。
async fn build_test_app() -> (axum::Router, tempfile::TempDir) {
    let url = format!(
        "sqlite:file:auth_flow_test_{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4()
    );
    let opts = SqliteConnectOptions::from_str(&url).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

    let user_repo = SqliteUserRepository::new(pool.clone());
    let session_repo = SqliteSessionRepository::new(pool);
    let hasher: Arc<dyn vpn_core::service::PasswordHasher> = Arc::new(Argon2Hasher::new());
    let tmp = tempfile::tempdir().unwrap();
    let issuer = JwtTokenIssuer::load_or_generate(tmp.path()).unwrap();
    let auth_service = Arc::new(AuthService {
        user_repo,
        session_repo,
        hasher,
        issuer,
        login_attempts: LoginAttempts::new(),
    });

    let state = AppState::new().with_auth_service(auth_service);
    (build_router(state), tmp)
}

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: Value,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let req = req.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn get_json(app: &axum::Router, uri: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().method("GET").uri(uri);
    if let Some(token) = bearer {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let req = req.body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn setup_status_initially_needs_setup() {
    let (app, _tmp) = build_test_app().await;
    let (status, body) = get_json(&app, "/api/v1/auth/setup-status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["needs_setup"], json!(true));
}

#[tokio::test]
async fn full_auth_flow_setup_login_protected_changepw() {
    let (app, _tmp) = build_test_app().await;

    // 1. first-time-setup 创建首位 admin
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/first-time-setup",
        json!({ "username": "admin", "email": "admin@example.com", "password": "secret123" }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "setup body: {body}");
    assert_eq!(body["code"], json!(0));

    // 2. setup-status 现在应为 false
    let (_, body) = get_json(&app, "/api/v1/auth/setup-status", None).await;
    assert_eq!(body["data"]["needs_setup"], json!(false));

    // 3. 重复 setup 应被拒绝（AlreadyInitialized=3005）
    let (_, body) = post_json(
        &app,
        "/api/v1/auth/first-time-setup",
        json!({ "username": "x", "email": "x@e.com", "password": "secret123" }),
        None,
    )
    .await;
    assert_eq!(body["code"], json!(3005));

    // 4. 登录
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "secret123" }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let access = body["data"]["access_token"].as_str().unwrap().to_string();
    let refresh = body["data"]["refresh_token"].as_str().unwrap().to_string();
    assert!(!access.is_empty());

    // 5. 无 token 访问受保护接口 → 401
    let (status, _) = get_json(&app, "/api/v1/admin/system/info", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 6. 带 token 访问受保护接口 → 200
    let (status, body) = get_json(&app, "/api/v1/admin/system/info", Some(&access)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["version"].is_string());

    // 7. refresh 换新 access token
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["data"]["access_token"].as_str().unwrap().is_empty());

    // 8. 修改密码（需要鉴权）
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/change-password",
        json!({ "old_password": "secret123", "new_password": "newsecret456" }),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "change-pw body: {body}");

    // 9. 旧密码登录失败（1001），新密码登录成功
    let (_, body) = post_json(
        &app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "secret123" }),
        None,
    )
    .await;
    assert_eq!(body["code"], json!(1001));

    let (status, _) = post_json(
        &app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "newsecret456" }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn login_with_wrong_password_returns_invalid_credentials() {
    let (app, _tmp) = build_test_app().await;
    post_json(
        &app,
        "/api/v1/auth/first-time-setup",
        json!({ "username": "admin", "email": "admin@example.com", "password": "secret123" }),
        None,
    )
    .await;

    let (_, body) = post_json(
        &app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "wrongpass1" }),
        None,
    )
    .await;
    assert_eq!(body["code"], json!(1001));
}
