//! Epic 3 端到端集成测试：admin 登录后对用户做 CRUD。
//!
//! 覆盖路由注册 + RequireAdmin 鉴权 + user_service 注入的完整链路。

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
    services::{Argon2Hasher, AuthService, JwtTokenIssuer, UserService},
    AppState,
};

/// 构造带 auth_service + user_service（内存 SQLite + 临时 RSA 密钥）的 Router。
async fn build_test_app() -> (axum::Router, tempfile::TempDir) {
    let url = format!(
        "sqlite:file:users_admin_test_{}?mode=memory&cache=shared",
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
        user_repo: user_repo.clone(),
        session_repo: session_repo.clone(),
        hasher: hasher.clone(),
        issuer,
        login_attempts: LoginAttempts::new(),
    });
    let user_service = Arc::new(UserService::new(user_repo, session_repo, hasher));

    let state = AppState::new()
        .with_auth_service(auth_service)
        .with_user_service(user_service);
    (build_router(state), tmp)
}

async fn req(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(token) = bearer {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// 初始化 admin 并登录，返回 (app, access_token, admin_id)。
async fn setup_admin() -> (axum::Router, tempfile::TempDir, String, String) {
    let (app, tmp) = build_test_app().await;
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/auth/first-time-setup",
        Some(json!({ "username": "admin", "email": "admin@example.com", "password": "secret123" })),
        None,
    )
    .await;
    let admin_id = body["data"]["user_id"].as_str().unwrap().to_string();
    let access = body["data"]["access_token"].as_str().unwrap().to_string();
    (app, tmp, access, admin_id)
}

#[tokio::test]
async fn unauthenticated_user_list_is_rejected() {
    let (app, _tmp, _access, _id) = setup_admin().await;
    let (status, _) = req(&app, "GET", "/api/v1/admin/users", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_list_disable_reset_delete_flow() {
    let (app, _tmp, access, _admin_id) = setup_admin().await;

    // 创建用户（不带密码 → 自动生成）
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({ "username": "alice", "email": "alice@example.com" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create body: {body}");
    let alice_id = body["data"]["user"]["id"].as_str().unwrap().to_string();
    let initial_pw = body["data"]["initial_password"].as_str().unwrap();
    assert!(initial_pw.len() >= 12);
    assert_eq!(body["data"]["user"]["must_change_password"], json!(true));
    assert_eq!(body["data"]["user"]["role"], json!("user"));

    // 重复用户名 → 3003
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({ "username": "alice", "email": "other@example.com" })),
        Some(&access),
    )
    .await;
    assert_eq!(body["code"], json!(3003));

    // 列表：含 admin + alice = 2
    let (status, body) = req(&app, "GET", "/api/v1/admin/users", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], json!(2));

    // 搜索 alice → 1
    let (_, body) = req(
        &app,
        "GET",
        "/api/v1/admin/users?search=alice",
        None,
        Some(&access),
    )
    .await;
    assert_eq!(body["data"]["total"], json!(1));
    assert_eq!(body["data"]["items"][0]["username"], json!("alice"));

    // 禁用 alice，然后 alice 无法登录（1004）
    let (status, _) = req(
        &app,
        "PATCH",
        &format!("/api/v1/admin/users/{alice_id}"),
        Some(json!({ "status": "disabled" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({ "username": "alice", "password": initial_pw })),
        None,
    )
    .await;
    assert_eq!(body["code"], json!(1004));

    // 重置密码 → 返回新密码且为新值
    let (status, body) = req(
        &app,
        "POST",
        &format!("/api/v1/admin/users/{alice_id}/reset-password"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new_pw = body["data"]["new_password"].as_str().unwrap();
    assert!(new_pw.len() >= 12);
    assert_ne!(new_pw, initial_pw);

    // 删除 alice → 列表回到 1
    let (status, _) = req(
        &app,
        "DELETE",
        &format!("/api/v1/admin/users/{alice_id}"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = req(&app, "GET", "/api/v1/admin/users", None, Some(&access)).await;
    assert_eq!(body["data"]["total"], json!(1));
}

#[tokio::test]
async fn admin_cannot_delete_or_reset_self() {
    let (app, _tmp, access, admin_id) = setup_admin().await;

    let (status, body) = req(
        &app,
        "DELETE",
        &format!("/api/v1/admin/users/{admin_id}"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], json!(2002));

    let (status, body) = req(
        &app,
        "POST",
        &format!("/api/v1/admin/users/{admin_id}/reset-password"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], json!(2002));
}
