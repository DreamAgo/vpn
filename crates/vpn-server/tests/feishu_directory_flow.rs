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
async fn build_test_app() -> (
    axum::Router,
    tempfile::TempDir,
    Arc<std::sync::Mutex<DirectoryUser>>,
) {
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

    let remote = Arc::new(std::sync::Mutex::new(DirectoryUser {
        union_id: "union-alice".into(),
        open_id: Some("open-alice".into()),
        user_id: None,
        name: "Alice".into(),
        email: "alice@example.com".into(),
        status: "active".into(),
    }));
    let directory = Arc::new(FeishuDirectoryService::new(
        pool,
        "app".into(),
        Default::default(),
        Arc::new(FakeApi(remote.clone())),
        None,
        None,
    ));
    let state = AppState::new()
        .with_feishu_directory_service(directory)
        .with_auth_service(auth_service)
        .with_user_service(user_service);
    (build_router(state), tmp, remote)
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

use vpn_server::services::feishu_directory_service::{
    DirectoryApi, DirectoryUser, FeishuDirectoryService,
};
struct FakeApi(Arc<std::sync::Mutex<DirectoryUser>>);
#[async_trait::async_trait]
impl DirectoryApi for FakeApi {
    async fn user(&self, _: &str, _: &str) -> vpn_core::Result<DirectoryUser> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn binding_is_admin_only_and_status_restrictions_leave_manual_accounts_usable() {
    let (app, _tmp, remote) = build_test_app().await;
    let (status, body) = req(
        &app,
        "POST",
        "/api/v1/auth/first-time-setup",
        Some(json!({"username":"admin", "email":"admin@example.com", "password":"secret123"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admin = body["data"]["access_token"].as_str().unwrap().to_string();
    let mut accounts = vec![];
    for name in ["alice", "manual"] {
        let (status, body) = req(
            &app,
            "POST",
            "/api/v1/admin/users",
            Some(json!({"username":name,"email":format!("{name}@example.com")})),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["data"]["user"]["id"].as_str().unwrap().to_string();
        let password = body["data"]["initial_password"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, body) = req(
            &app,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":name,"password":password})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        accounts.push((
            id,
            password,
            body["data"]["access_token"].as_str().unwrap().to_string(),
            body["data"]["refresh_token"].as_str().unwrap().to_string(),
        ));
    }
    let lookup = "/api/v1/admin/integrations/feishu/users/lookup";
    let input = json!({"user_id":"open-alice","id_type":"open_id"});
    assert_eq!(
        req(&app, "POST", lookup, Some(input.clone()), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        req(
            &app,
            "POST",
            lookup,
            Some(input.clone()),
            Some(&accounts[0].2)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        req(&app, "POST", lookup, Some(input.clone()), Some(&admin))
            .await
            .0,
        StatusCode::OK
    );
    // 预览不会建立绑定，批量同步此时仍为零。
    let (_, body) = req(
        &app,
        "POST",
        "/api/v1/admin/integrations/feishu/users/sync",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(body["data"], 0);
    let (status, body) = req(
        &app,
        "POST",
        &format!("/api/v1/admin/users/{}/feishu-binding", accounts[0].0),
        Some(input),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    remote.lock().unwrap().status = "frozen".into();
    let (status, body) = req(
        &app,
        "POST",
        &format!("/api/v1/admin/users/{}/feishu-sync", accounts[0].0),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (index, name) in ["alice", "manual"].into_iter().enumerate() {
        let (_, password, access, refresh) = &accounts[index];
        let (status, body) = req(
            &app,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":name,"password":password})),
            None,
        )
        .await;
        if index == 0 {
            assert_ne!(status, StatusCode::OK, "{body}");
            assert_eq!(body["code"], vpn_core::AppError::AccountDisabled.code());
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        let (status, body) = req(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            Some(json!({"refresh_token":refresh})),
            None,
        )
        .await;
        if index == 0 {
            assert_ne!(status, StatusCode::OK, "{body}");
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        let (_, body) = req(&app, "GET", "/api/v1/admin/users", None, Some(access)).await;
        if index == 0 {
            assert_eq!(body["code"], vpn_core::AppError::AccountDisabled.code());
        } else {
            assert_ne!(body["code"], vpn_core::AppError::AccountDisabled.code());
        }
    }
}
