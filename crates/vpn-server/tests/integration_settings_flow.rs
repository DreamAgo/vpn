//! 管理员飞书集成设置 API：鉴权、脱敏与原子失败。

use std::{str::FromStr, sync::Arc};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tower::ServiceExt;
use vpn_server::{
    build_router,
    config::{
        DataPlaneSettingsSeed, FeishuApprovalConfig, FeishuApprovalOptionsConfig, FeishuConfig,
        NetworkSettingsSeed,
    },
    ratelimit::LoginAttempts,
    repositories::{
        SqlitePeerRepository, SqliteSessionRepository, SqliteSystemConfigRepository,
        SqliteUserRepository,
    },
    services::{
        Argon2Hasher, AuthService, IntegrationSettingsService, JwtTokenIssuer,
        NetworkSettingsService, UserService,
    },
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
        "sqlite:file:integration_settings_flow_{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4()
    );
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let repo = SqliteSystemConfigRepository::new(pool.clone());
    let integration = Arc::new(
        IntegrationSettingsService::load_or_seed(
            repo,
            pool.clone(),
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        )
        .await
        .unwrap(),
    );
    let network = Arc::new(
        NetworkSettingsService::load_or_seed(
            SqliteSystemConfigRepository::new(pool.clone()),
            SqlitePeerRepository::new(pool.clone()),
            &DataPlaneSettingsSeed {
                vpn_subnet: Some("10.8.0.0/24".into()),
                vpn_listen_port: Some("51820".into()),
                vpn_endpoint: Some("vpn.example.com:51820".into()),
                wg_backend: Some("kernel".into()),
                wg_interface: Some("wg0".into()),
                obfs_enabled: Some("false".into()),
                obfs_mode: Some("low-overhead-v1".into()),
                obfs_bind_addr: Some("0.0.0.0:47358".into()),
                obfs_endpoint: Some("vpn.example.com:47358".into()),
                obfs_path_mtu: Some("1500".into()),
                server_routes: Some(String::new()),
                ..Default::default()
            },
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap(),
    );
    let user_repo = SqliteUserRepository::new(pool.clone());
    let session_repo = SqliteSessionRepository::new(pool);
    let hasher: Arc<dyn vpn_core::service::PasswordHasher> = Arc::new(Argon2Hasher::new());
    let temp = tempfile::tempdir().unwrap();
    let auth = Arc::new(AuthService {
        user_repo: user_repo.clone(),
        session_repo: session_repo.clone(),
        hasher: hasher.clone(),
        issuer: JwtTokenIssuer::load_or_generate(temp.path()).unwrap(),
        login_attempts: LoginAttempts::new(),
    });
    let app = build_router(
        AppState::new()
            .with_auth_service(auth)
            .with_user_service(Arc::new(UserService::new(user_repo, session_repo, hasher)))
            .with_integration_settings_service(integration)
            .with_network_settings_service(network),
    );
    let (_, setup) = request(
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
    (
        app,
        temp,
        setup["data"]["access_token"].as_str().unwrap().into(),
    )
}

fn valid_request(secret: &str, redirect: &str) -> Value {
    json!({
        "feishu_login": {
            "enabled": true,
            "app_id": "cli_test",
            "redirect_uri": redirect,
            "app_secret": { "value": secret, "clear": false }
        },
        "feishu_approval": {
            "enabled": false,
            "approval_code": null,
            "group_control_id": null,
            "expiry_control_id": null,
            "reason_control_id": null,
            "verification_token": { "value": null, "clear": false },
            "encrypt_key": { "value": null, "clear": false }
        },
        "external_options": { "token": { "value": null, "clear": false } }
    })
}

#[tokio::test]
async fn admin_update_is_redacted_and_invalid_update_is_atomic() {
    let (app, _temp, admin) = setup().await;
    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/admin/integrations/settings",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, created) = request(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(json!({ "username": "member", "email": "member@example.com" })),
        Some(&admin),
    )
    .await;
    let password = created["data"]["initial_password"].as_str().unwrap();
    let (_, login) = request(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({ "username": "member", "password": password })),
        None,
    )
    .await;
    let member = login["data"]["access_token"].as_str().unwrap();
    for method in ["GET", "POST"] {
        let uri = "/api/v1/admin/integrations/feishu/approval-subscription";
        let (status, _) = request(&app, method, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = request(&app, method, uri, None, Some(member)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let subscription_uri = "/api/v1/admin/integrations/feishu/approval-subscription";
    let (status, subscription) = request(&app, "GET", subscription_uri, None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(subscription["data"]["can_subscribe"], false);
    assert!(subscription["data"]["last_success_at"].is_null());
    let (status, _) = request(&app, "POST", subscription_uri, None, Some(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/admin/integrations/settings",
        None,
        Some(member),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, updated) = request(
        &app,
        "PUT",
        "/api/v1/admin/integrations/settings",
        Some(valid_request(
            "never-return-this",
            "https://vpn.example.com/api/v1/auth/feishu/callback",
        )),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated["data"]["desired"]["feishu_login"]["app_secret_set"],
        true
    );
    assert_eq!(updated["data"]["applied"]["feishu_login"]["enabled"], false);
    assert!(!updated.to_string().contains("never-return-this"));
    assert_eq!(updated["data"]["restart_required"], true);

    let (status, _) = request(&app, "POST", subscription_uri, None, Some(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, subscription) = request(&app, "GET", subscription_uri, None, Some(&admin)).await;
    assert_eq!(subscription["data"]["can_subscribe"], false);
    assert!(subscription["data"]["last_success_at"].is_null());

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/admin/integrations/settings",
        Some(valid_request(
            "replacement",
            "https://vpn.example.com/api/v1/auth/feishu/callback?bad=1",
        )),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, after) = request(
        &app,
        "GET",
        "/api/v1/admin/integrations/settings",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(after["data"], updated["data"]);
    assert!(!after.to_string().contains("never-return-this"));
    assert!(!after.to_string().contains("replacement"));
}
