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

    // 查询审计日志（默认最近 7 天）。
    let (status, body) = req(&app, "GET", "/api/v1/admin/audit-logs", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK, "audit query body: {body}");
    let items = body["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty(), "应有审计记录: {body}");

    // 应能找到 user_create / resource 正确。
    let has_user_create = items.iter().any(|it| {
        it["action"] == json!("user_create")
            && it["resource"]
                .as_str()
                .is_some_and(|r| r.starts_with("users/"))
            && it["username"] == "admin"
            && it["status_code"] == json!(200)
    });
    assert_eq!(
        items
            .iter()
            .filter(|it| it["action"] == "user_create")
            .count(),
        1
    );
    assert!(!body.to_string().contains("$argon2"));
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

fn signed_expired_token(tmp: &tempfile::TempDir, user_id: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let key = std::fs::read(tmp.path().join("jwt_private.pem")).unwrap();
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &json!({"sub":user_id,"role":"user","exp":now-60,"iat":now-960}),
        &jsonwebtoken::EncodingKey::from_rsa_pem(&key).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn routine_expiration_is_not_audited_but_invalid_credentials_are() {
    let (app, tmp, access, admin_id) = setup_admin().await;
    let expired = signed_expired_token(&tmp, &admin_id);
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({})),
        Some(&expired),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
    assert_eq!(body["message"], "Token 过期或无效");
    let query = "/api/v1/admin/audit-logs?action=auth.rejected";
    let (_, logs) = req(&app, "GET", query, None, Some(&access)).await;
    assert_eq!(logs["data"]["total"], 0);
    let (_, health) = req(
        &app,
        "GET",
        "/api/v1/admin/audit-health",
        None,
        Some(&access),
    )
    .await;
    assert_eq!(health["data"]["access_expirations"], 1);
    for _ in 0..3 {
        let (status, body) = req(
            &app,
            "POST",
            "/api/v1/peers/heartbeat",
            Some(json!({})),
            Some("forged.jwt.token"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], 1002);
    }
    let (_, logs) = req(&app, "GET", query, None, Some(&access)).await;
    let rows = logs["data"]["items"].as_array().unwrap();
    // A wall-clock bucket boundary can legitimately produce two groups.
    let count: u64 = rows
        .iter()
        .map(|row| {
            assert!(row["user_id"].is_null());
            let metadata: Value = serde_json::from_str(row["metadata"].as_str().unwrap()).unwrap();
            assert_eq!(metadata["reason_code"], 1002);
            metadata["occurrences"].as_u64().unwrap()
        })
        .sum();
    assert_eq!(count, 3);
    assert!(!logs.to_string().contains("forged.jwt.token"));
}

#[tokio::test]
async fn disabled_accounts_and_failed_refresh_remain_auditable() {
    let (app, tmp, access, _) = setup_admin().await;
    let (status, user) = req(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({"username":"disabled-user","email":"disabled@example.com"})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    let id = user["data"]["user"]["id"]
        .as_str()
        .or_else(|| user["data"]["id"].as_str())
        .unwrap();
    let (status, body) = req(
        &app,
        "PATCH",
        &format!("/api/v1/admin/users/{id}"),
        Some(json!({"status":"disabled"})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expired = signed_expired_token(&tmp, id);
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({})),
        Some(&expired),
    )
    .await;
    assert_eq!(body["code"], 1004);
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token":"invalid-secret"})),
        None,
    )
    .await;
    assert_eq!(body["code"], 1002);
    let (_, logs) = req(
        &app,
        "GET",
        "/api/v1/admin/audit-logs?action=auth.rejected",
        None,
        Some(&access),
    )
    .await;
    let rows = logs["data"]["items"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{logs}");
    assert!(rows
        .iter()
        .any(|row| row["user_id"] == id && row["resource"] == "/api/v1/peers/heartbeat"));
    assert!(rows
        .iter()
        .any(|row| row["resource"] == "/api/v1/auth/refresh"));
    assert!(!logs.to_string().contains("invalid-secret"));
}

#[tokio::test]
async fn rejection_aggregation_preserves_counts_and_separates_keys_and_windows() {
    use vpn_server::repositories::AuditLogEntry;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let audit = AuditService::new(SqliteAuditLogRepository::new(pool.clone()));
    let entry = AuditLogEntry {
        action: "auth.rejected".into(),
        resource: "/api/v1/peers/heartbeat".into(),
        ip_addr: Some("192.0.2.1".into()),
        status_code: Some(401),
        metadata: Some(
            json!({"outcome":"failed","reason_code":1002,"request_id":"first"}).to_string(),
        ),
        ..Default::default()
    };
    let mut jobs = Vec::new();
    for n in 0..8 {
        let audit = audit.clone();
        let entry = entry.clone();
        jobs.push(tokio::spawn(async move {
            audit.log_rejection(entry, 300_000 + n).await
        }));
    }
    for job in jobs {
        job.await.unwrap();
    }
    let rows: Vec<(String,)> = sqlx::query_as("SELECT metadata FROM audit_logs")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let data: Value = serde_json::from_str(&rows[0].0).unwrap();
    assert_eq!(data["occurrences"], 8);
    assert_eq!(data["first_seen_at"], 300_000);
    assert_eq!(data["last_seen_at"], 300_007);
    // Recreating the service must not reset the persisted group count.
    let audit = AuditService::new(SqliteAuditLogRepository::new(pool.clone()));
    audit.log_rejection(entry.clone(), 300_020).await;
    audit.log_rejection(entry.clone(), 600_000).await;
    let mut other = entry.clone();
    other.ip_addr = Some("192.0.2.2".into());
    audit.log_rejection(other, 300_030).await;
    let mut other = entry;
    other.metadata = Some(json!({"outcome":"failed","reason_code":1007}).to_string());
    audit.log_rejection(other, 300_040).await;
    let (groups, count): (i64, i64) = sqlx::query_as(
        "SELECT count(*),sum(json_extract(metadata,'$.occurrences')) FROM audit_logs",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((groups, count), (4, 12));
}

#[tokio::test]
async fn revoked_refresh_is_attributed_without_recording_credentials() {
    let (app, _tmp, access, admin_id) = setup_admin().await;
    let (_, login) = req(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"admin","password":"secret123"})),
        None,
    )
    .await;
    let refresh = login["data"]["refresh_token"].as_str().unwrap();
    let (status, _) = req(
        &app,
        "POST",
        "/api/v1/auth/logout",
        Some(json!({"refresh_token":refresh})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token":refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
    let (_, logs) = req(
        &app,
        "GET",
        "/api/v1/admin/audit-logs?action=auth.rejected",
        None,
        Some(&access),
    )
    .await;
    assert_eq!(logs["data"]["total"], 1);
    let row = &logs["data"]["items"][0];
    assert_eq!(row["user_id"], admin_id);
    assert_eq!(row["username"], "admin");
    assert!(!logs.to_string().contains(refresh));
}
