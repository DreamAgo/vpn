//! Epic 5 端到端集成测试：审计日志查询 + admin peer 列表 + 强制下线。
//!
//! 覆盖：审计中间件写入 → 查询；admin peer JOIN username；强制下线后心跳被拒。

use std::str::FromStr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ipnet::Ipv4Net;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tower::ServiceExt;

use vpn_server::{
    build_router,
    ratelimit::LoginAttempts,
    repositories::{
        SqliteAuditLogRepository, SqlitePeerRepository, SqliteSessionRepository,
        SqliteSystemConfigRepository, SqliteUserRepository,
    },
    services::{
        build_peer_service, Argon2Hasher, AuditService, AuthService, JwtTokenIssuer, UserService,
    },
    AppState,
};

/// 构造带全套 service（内存 SQLite + 临时 JWT 密钥）的 Router。
async fn build_test_app() -> (axum::Router, tempfile::TempDir) {
    let url = format!(
        "sqlite:file:audit_admin_test_{}?mode=memory&cache=shared",
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
        user_repo: user_repo.clone(),
        session_repo: session_repo.clone(),
        hasher: hasher.clone(),
        issuer,
        login_attempts: LoginAttempts::new(),
    });
    let user_service = Arc::new(UserService::new(user_repo, session_repo, hasher));

    let peer_repo = SqlitePeerRepository::new(pool.clone());
    let config_repo = SqliteSystemConfigRepository::new(pool.clone());
    let subnet: Ipv4Net = "10.8.0.0/24".parse().unwrap();
    let peer_service = Arc::new(
        build_peer_service(
            peer_repo,
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap(),
    );

    let audit_service = Arc::new(AuditService::new(SqliteAuditLogRepository::new(pool)));

    let state = AppState::new()
        .with_auth_service(auth_service)
        .with_user_service(user_service)
        .with_peer_service(peer_service)
        .with_audit_service(audit_service);
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

/// 初始化 admin 并登录，返回 (app, tmp, access_token, admin_id)。
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

/// 审计是异步 spawn 写入，给后台任务一点时间落库。
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

#[tokio::test]
async fn write_ops_are_audited_and_queryable() {
    let (app, _tmp, access, _admin_id) = setup_admin().await;

    // 做一个写操作：创建用户 → 应产生 user_create 审计。
    let (status, _) = req(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({ "username": "alice", "email": "alice@example.com" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    settle().await;

    // 查询审计日志（默认最近 7 天）。
    let (status, body) = req(&app, "GET", "/api/v1/admin/audit-logs", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK, "audit query body: {body}");
    let items = body["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty(), "应有审计记录: {body}");

    // 应能找到 user_create / resource 正确。
    let has_user_create = items.iter().any(|it| {
        it["action"] == json!("user_create")
            && it["resource"] == json!("/api/v1/admin/users")
            && it["status_code"] == json!(200)
    });
    assert!(has_user_create, "缺少 user_create 审计: {body}");

    // 应有登录成功审计（first-time-setup 不走 login，但我们显式登录一次）。
    let (_, _) = req(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({ "username": "admin", "password": "secret123" })),
        None,
    )
    .await;
    settle().await;
    let (_, body) = req(
        &app,
        "GET",
        "/api/v1/admin/audit-logs?action=login_success",
        None,
        Some(&access),
    )
    .await;
    assert!(
        body["data"]["total"].as_u64().unwrap() >= 1,
        "缺少 login_success 审计: {body}"
    );
}

#[tokio::test]
async fn failed_login_is_audited() {
    let (app, _tmp, access, _id) = setup_admin().await;
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({ "username": "admin", "password": "wrongpass" })),
        None,
    )
    .await;
    assert_eq!(body["code"], json!(1001));
    settle().await;

    let (_, body) = req(
        &app,
        "GET",
        "/api/v1/admin/audit-logs?action=login_failed",
        None,
        Some(&access),
    )
    .await;
    assert!(
        body["data"]["total"].as_u64().unwrap() >= 1,
        "缺少 login_failed 审计: {body}"
    );
}

#[tokio::test]
async fn admin_peer_list_and_force_remove_flow() {
    let (app, _tmp, access, _admin_id) = setup_admin().await;

    // admin 自己注册一个 peer（任何登录用户均可注册）。
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/register",
        Some(json!({ "wg_public_key": "PK_TEST_1", "device_name": "AdminLaptop" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register body: {body}");

    // 心跳一次（正常）。
    let (status, _) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // admin peer 列表：应含 username。
    let (status, body) = req(&app, "GET", "/api/v1/admin/peers", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK, "peers body: {body}");
    assert_eq!(body["data"]["total"], json!(1));
    let item = &body["data"]["items"][0];
    assert_eq!(item["username"], json!("admin"));
    assert_eq!(item["device_name"], json!("AdminLaptop"));
    let peer_id = item["id"].as_str().unwrap().to_string();

    // 强制下线。
    let (status, _) = req(
        &app,
        "DELETE",
        &format!("/api/v1/admin/peers/{peer_id}"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 列表中该 peer 状态应为 force_removed。
    let (_, body) = req(
        &app,
        "GET",
        "/api/v1/admin/peers?status=force_removed",
        None,
        Some(&access),
    )
    .await;
    assert_eq!(body["data"]["total"], json!(1));
    assert_eq!(body["data"]["items"][0]["status"], json!("force_removed"));

    // 强制下线后心跳被拒（401）。
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "heartbeat body: {body}");
}

#[tokio::test]
async fn audit_logs_require_admin() {
    let (app, _tmp, _access, _id) = setup_admin().await;
    let (status, _) = req(&app, "GET", "/api/v1/admin/audit-logs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
