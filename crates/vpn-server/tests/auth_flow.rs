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
    repositories::{
        SqlitePeerRepository, SqliteSessionRepository, SqliteSystemConfigRepository,
        SqliteUserRepository,
    },
    services::{build_peer_service, Argon2Hasher, AuthService, JwtTokenIssuer},
    AppState,
};

/// 构造带真实 AuthService（内存 SQLite + 临时 RSA 密钥）的 Router。
async fn build_test_app() -> (axum::Router, tempfile::TempDir, sqlx::SqlitePool) {
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
    let session_repo = SqliteSessionRepository::new(pool.clone());
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

    // 受保护接口 /admin/system/info 现读取真实 peer_service，故测试也需装配（Noop 后端）。
    let peer_repo = SqlitePeerRepository::new(pool.clone());
    let config_repo = SqliteSystemConfigRepository::new(pool.clone());
    let subnet = "10.8.0.0/24".parse().unwrap();
    let peer_service = build_peer_service(
        peer_repo,
        &config_repo,
        subnet,
        "vpn.example.com:51820".to_string(),
    )
    .await
    .unwrap();

    let state = AppState::new()
        .with_auth_service(auth_service)
        .with_peer_service(Arc::new(peer_service));
    (build_router(state), tmp, pool)
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
    let (app, _tmp, _pool) = build_test_app().await;
    let (status, body) = get_json(&app, "/api/v1/auth/setup-status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["needs_setup"], json!(true));
}

#[tokio::test]
async fn full_auth_flow_setup_login_protected_changepw() {
    let (app, _tmp, _pool) = build_test_app().await;

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
    let (app, _tmp, _pool) = build_test_app().await;
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

async fn create_admin_and_login(app: &axum::Router) -> (String, String) {
    post_json(
        app,
        "/api/v1/auth/first-time-setup",
        json!({ "username": "admin", "email": "admin@example.com", "password": "secret123" }),
        None,
    )
    .await;
    let (status, body) = post_json(
        app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "secret123" }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login body: {body}");
    (
        body["data"]["access_token"].as_str().unwrap().to_string(),
        body["data"]["refresh_token"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn refresh_slides_only_the_current_session_expiry() {
    use vpn_server::services::token_issuer::hash_refresh_token;

    let (app, _tmp, pool) = build_test_app().await;
    let (_, first_refresh) = create_admin_and_login(&app).await;
    let (_, second_body) = post_json(
        &app,
        "/api/v1/auth/login",
        json!({ "username": "admin", "password": "secret123" }),
        None,
    )
    .await;
    let second_refresh = second_body["data"]["refresh_token"].as_str().unwrap();
    let first_hash = hash_refresh_token(&first_refresh);
    let second_hash = hash_refresh_token(second_refresh);
    let now = chrono::Utc::now().timestamp_millis();
    let controlled_expiry = now + 60_000;
    sqlx::query("UPDATE sessions SET expires_at = ?1 WHERE refresh_token_hash IN (?2, ?3)")
        .bind(controlled_expiry)
        .bind(&first_hash)
        .bind(&second_hash)
        .execute(&pool)
        .await
        .unwrap();

    let session_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let first_refresh_started = chrono::Utc::now().timestamp_millis();
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": &first_refresh }),
        None,
    )
    .await;
    let first_refresh_finished = chrono::Utc::now().timestamp_millis();
    assert_eq!(status, StatusCode::OK, "refresh body: {body}");
    let first_expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(&first_hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    let second_expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(second_hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    let ttl_ms = chrono::Duration::days(30).num_milliseconds();
    assert!(first_expiry >= first_refresh_started + ttl_ms);
    assert!(first_expiry <= first_refresh_finished + ttl_ms);
    assert_eq!(second_expiry, controlled_expiry);

    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": first_refresh }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "second refresh body: {body}");
    let repeated_expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(&first_hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    let session_count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(repeated_expiry >= first_expiry);
    assert_eq!(session_count_after, session_count_before);
}

#[tokio::test]
async fn disabled_user_cannot_refresh_or_extend_session() {
    use vpn_server::services::token_issuer::hash_refresh_token;

    let (app, _tmp, pool) = build_test_app().await;
    let (_, refresh) = create_admin_and_login(&app).await;
    let hash = hash_refresh_token(&refresh);
    let expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("UPDATE users SET status = 'disabled'")
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "refresh body: {body}");
    assert_eq!(body["code"], json!(1004));
    let unchanged: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, expiry);
}

#[tokio::test]
async fn logout_revocation_cannot_be_refreshed_or_renewed() {
    use vpn_server::services::token_issuer::hash_refresh_token;

    let (app, _tmp, pool) = build_test_app().await;
    let (access, refresh) = create_admin_and_login(&app).await;
    let hash = hash_refresh_token(&refresh);
    let expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (status, _) = post_json(
        &app,
        "/api/v1/auth/logout",
        json!({ "refresh_token": refresh.clone() }),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "refresh body: {body}");
    assert_eq!(body["code"], json!(1002));
    let row: (i64, Option<i64>) =
        sqlx::query_as("SELECT expires_at, revoked_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, expiry);
    assert!(row.1.is_some());
}

#[tokio::test]
async fn revocation_between_lookup_and_renewal_returns_token_expired() {
    use vpn_server::services::token_issuer::hash_refresh_token;

    let (app, _tmp, pool) = build_test_app().await;
    let (_, refresh) = create_admin_and_login(&app).await;
    let hash = hash_refresh_token(&refresh);
    let expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    // 在 AuthService 完成 active session / user 查询后，模拟管理员并发撤销。
    // RAISE(IGNORE) 令外层续期 UPDATE 报告 0 行，从而验证 service 不会签发 Access Token。
    sqlx::query(
        r#"CREATE TRIGGER simulate_concurrent_revoke
           BEFORE UPDATE OF expires_at ON sessions
           BEGIN
             UPDATE sessions SET revoked_at = 1 WHERE id = OLD.id;
             SELECT RAISE(IGNORE);
           END"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "refresh body: {body}");
    assert_eq!(body["code"], json!(1002));
    let row: (i64, Option<i64>) =
        sqlx::query_as("SELECT expires_at, revoked_at FROM sessions WHERE refresh_token_hash = ?1")
            .bind(hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, expiry);
    assert_eq!(row.1, Some(1));
}
