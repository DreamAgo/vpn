//! Epic 4 端到端集成测试：登录用户的 peer 数据平面（注册 / 心跳 / 配置 / 注销）。
//!
//! 覆盖路由注册 + CurrentUser 鉴权 + peer_service（Noop control）注入的完整链路。

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
    services::{build_peer_service, Argon2Hasher, AuthService, JwtTokenIssuer, UserService},
    AppState,
};

async fn build_test_app() -> (axum::Router, tempfile::TempDir) {
    let url = format!(
        "sqlite:file:peer_flow_test_{}?mode=memory&cache=shared",
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
    let config_repo = SqliteSystemConfigRepository::new(pool);
    let subnet: ipnet::Ipv4Net = "10.8.0.0/24".parse().unwrap();
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

    let state = AppState::new()
        .with_auth_service(auth_service)
        .with_user_service(user_service)
        .with_peer_service(peer_service);
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

/// 初始化首个 admin（普通登录用户也可，但 admin 就是个 user 账号），登录拿 token。
async fn setup_user() -> (axum::Router, tempfile::TempDir, String) {
    let (app, tmp) = build_test_app().await;
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/auth/first-time-setup",
        Some(json!({ "username": "alice", "email": "alice@example.com", "password": "secret123" })),
        None,
    )
    .await;
    let access = body["data"]["access_token"].as_str().unwrap().to_string();
    (app, tmp, access)
}

#[tokio::test]
async fn unauthenticated_register_is_rejected() {
    let (app, _tmp, _access) = setup_user().await;
    let (status, _) = req(
        &app,
        "POST",
        "/api/v1/peers/register",
        Some(json!({ "wg_public_key": "PK1", "device_name": "MBP" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn full_peer_lifecycle() {
    let (app, _tmp, access) = setup_user().await;

    // register → 分配 10.8.0.2
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/register",
        Some(json!({ "wg_public_key": "PK1", "device_name": "MBP", "os_info": "macOS" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register body: {body}");
    let vpn_ip = body["data"]["vpn_ip"].as_str().unwrap().to_string();
    assert_eq!(vpn_ip, "10.8.0.2");
    assert_eq!(
        body["data"]["server_endpoint"],
        json!("vpn.example.com:51820")
    );
    assert_eq!(body["data"]["vpn_subnet"], json!("10.8.0.0/24"));
    let server_pub = body["data"]["server_public_key"].as_str().unwrap();
    assert_eq!(server_pub.len(), 44);

    // 重复 register（换公钥）→ 同 IP 复用
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/register",
        Some(json!({ "wg_public_key": "PK2", "device_name": "MBP-2" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["vpn_ip"], json!("10.8.0.2"));

    // heartbeat → 200
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({ "endpoint": "1.2.3.4:51820" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "heartbeat body: {body}");
    assert_eq!(body["code"], json!(0));

    // GET config → text/plain + Content-Disposition + [Interface]
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/peers/me/config")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.starts_with("text/plain"));
    let disposition = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(disposition, "attachment; filename=\"vpn.conf\"");
    let text = String::from_utf8(
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("[Interface]"));
    assert!(text.contains("Address = 10.8.0.2/24"));
    assert!(text.contains("[Peer]"));
    assert!(text.contains(server_pub));

    // DELETE me → 200
    let (status, _) = req(&app, "DELETE", "/api/v1/peers/me", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK);

    // 注销后 heartbeat → PeerNotFound (3002, HTTP 404)
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/heartbeat",
        Some(json!({})),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], json!(3002));

    // 注销后再 register → 拿到新 IP（旧 IP 暂不释放）
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/peers/register",
        Some(json!({ "wg_public_key": "PK3", "device_name": "MBP-3" })),
        Some(&access),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["vpn_ip"], json!("10.8.0.3"));
}
