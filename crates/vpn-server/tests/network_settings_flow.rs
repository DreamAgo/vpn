//! 管理员网络参数 API：鉴权、原子校验与持久化返回值。

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
    config::NetworkSettingsSeed,
    ratelimit::LoginAttempts,
    repositories::{SqliteSessionRepository, SqliteSystemConfigRepository, SqliteUserRepository},
    services::{Argon2Hasher, AuthService, JwtTokenIssuer, NetworkSettingsService, UserService},
    AppState,
};

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn setup() -> (axum::Router, tempfile::TempDir, String) {
    let url = format!(
        "sqlite:file:network_settings_flow_{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4()
    );
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

    let user_repo = SqliteUserRepository::new(pool.clone());
    let session_repo = SqliteSessionRepository::new(pool.clone());
    let hasher: Arc<dyn vpn_core::service::PasswordHasher> = Arc::new(Argon2Hasher::new());
    let temp = tempfile::tempdir().unwrap();
    let auth_service = Arc::new(AuthService {
        user_repo: user_repo.clone(),
        session_repo: session_repo.clone(),
        hasher: hasher.clone(),
        issuer: JwtTokenIssuer::load_or_generate(temp.path()).unwrap(),
        login_attempts: LoginAttempts::new(),
    });
    let user_service = Arc::new(UserService::new(user_repo, session_repo, hasher));
    let network_settings_service = Arc::new(
        NetworkSettingsService::load_or_seed(
            SqliteSystemConfigRepository::new(pool),
            &NetworkSettingsSeed::default(),
            None,
        )
        .await
        .unwrap(),
    );
    let app = build_router(
        AppState::new()
            .with_auth_service(auth_service)
            .with_user_service(user_service)
            .with_network_settings_service(network_settings_service),
    );
    let (_, setup_body) = request(
        &app,
        "POST",
        "/api/v1/auth/first-time-setup",
        Some(json!({
            "username": "admin",
            "email": "admin@example.com",
            "password": "secret123"
        })),
        None,
    )
    .await;
    let admin_token = setup_body["data"]["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    (app, temp, admin_token)
}

#[tokio::test]
async fn admin_can_update_but_invalid_request_keeps_previous_value() {
    let (app, _temp, admin_token) = setup().await;
    let (status, initial) = request(
        &app,
        "GET",
        "/api/v1/admin/network/settings",
        None,
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["data"]["default_mtu"], json!(1360));

    let (status, updated) = request(
        &app,
        "PUT",
        "/api/v1/admin/network/settings",
        Some(json!({ "mode": "auto", "default_mtu": 1340, "min_mtu": 1280, "max_mtu": 1400 })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["mode"], json!("auto"));

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/admin/network/settings",
        Some(json!({ "mode": "fixed", "default_mtu": 1300, "min_mtu": 1400, "max_mtu": 1420 })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/admin/network/settings",
        Some(json!({ "mode": "bogus", "default_mtu": 1360, "min_mtu": 1280, "max_mtu": 1420 })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, after) = request(
        &app,
        "GET",
        "/api/v1/admin/network/settings",
        None,
        Some(&admin_token),
    )
    .await;
    assert_eq!(after["data"], updated["data"]);
}

#[tokio::test]
async fn non_admin_cannot_read_or_modify_network_settings() {
    let (app, _temp, admin_token) = setup().await;
    let (_, created) = request(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({ "username": "alice", "email": "alice@example.com" })),
        Some(&admin_token),
    )
    .await;
    let password = created["data"]["initial_password"].as_str().unwrap();
    let (_, logged_in) = request(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({ "username": "alice", "password": password })),
        None,
    )
    .await;
    let user_token = logged_in["data"]["access_token"].as_str().unwrap();

    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(
            || json!({ "mode": "fixed", "default_mtu": 1360, "min_mtu": 1280, "max_mtu": 1420 }),
        );
        let (status, _) = request(
            &app,
            method,
            "/api/v1/admin/network/settings",
            body,
            Some(user_token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
