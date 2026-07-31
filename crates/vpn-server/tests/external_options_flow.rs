//! 飞书审批外部选项端到端测试。

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
    repositories::SqliteSubnetRepository,
    services::{ExternalOptionsService, SubnetExternalOptionProvider, SubnetService},
    AppState,
};

async fn build_test_app(token: Option<&str>, subnet_count: usize) -> axum::Router {
    let url = format!(
        "sqlite:file:external_options_test_{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4()
    );
    let options = SqliteConnectOptions::from_str(&url).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

    let subnets = Arc::new(SubnetService::new(SqliteSubnetRepository::new(pool)));
    for index in 0..subnet_count {
        subnets
            .create(&format!("网段 {index:03}"), &format!("10.{index}.0.0/16"))
            .await
            .unwrap();
    }

    let mut external_options = ExternalOptionsService::new(token.map(str::to_string));
    external_options
        .register(
            "subnets",
            Arc::new(SubnetExternalOptionProvider::new(subnets.clone())),
        )
        .unwrap();
    build_router(
        AppState::new()
            .with_subnet_service(subnets)
            .with_external_options_service(Arc::new(external_options)),
    )
}

async fn post(app: &axum::Router, source: &str, body: Value) -> (StatusCode, Value) {
    post_raw(app, source, "application/json", &body.to_string()).await
}

async fn post_raw(
    app: &axum::Router,
    source: &str,
    content_type: &str,
    body: &str,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/integrations/feishu/approval-options/{source}"
                ))
                .header("content-type", content_type)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn external_options_returns_stable_subnet_option_contract() {
    let app = build_test_app(Some("approval-secret"), 1).await;
    let (status, body) = post(
        &app,
        "subnets",
        json!({ "token": "approval-secret", "locale": "zh_cn" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], json!(0));
    let option = &body["data"]["result"]["options"][0];
    let id = option["id"].as_str().unwrap();
    let value = option["value"].as_str().unwrap();
    assert!(!id.is_empty());
    assert_eq!(value, format!("@i18n@subnets_{id}"));
    assert_eq!(
        body["data"]["result"]["i18nResources"][0]["texts"][value],
        json!("网段 000（10.0.0.0/16）")
    );
}

#[tokio::test]
async fn external_options_supports_search_and_signed_pagination() {
    let app = build_test_app(Some("approval-secret"), 55).await;
    let (_, first) = post(&app, "subnets", json!({ "token": "approval-secret" })).await;
    assert_eq!(
        first["data"]["result"]["options"].as_array().unwrap().len(),
        50
    );
    assert_eq!(first["data"]["result"]["hasMore"], json!(true));

    let page_token = first["data"]["result"]["nextPageToken"].as_str().unwrap();
    let (_, second) = post(
        &app,
        "subnets",
        json!({ "token": "approval-secret", "page_token": page_token }),
    )
    .await;
    assert_eq!(
        second["data"]["result"]["options"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(second["data"]["result"]["hasMore"], json!(false));

    let (_, searched) = post(
        &app,
        "subnets",
        json!({ "token": "approval-secret", "query": "10.23." }),
    )
    .await;
    assert_eq!(
        searched["data"]["result"]["options"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn external_options_rejects_bad_auth_source_cursor_and_missing_config() {
    let app = build_test_app(Some("approval-secret"), 1).await;
    let (status, body) = post(&app, "subnets", json!({ "token": "wrong" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], json!(40100));

    let (status, body) = post(&app, "unknown", json!({ "token": "approval-secret" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], json!(40400));

    let (status, body) = post(
        &app,
        "subnets",
        json!({ "token": "approval-secret", "page_token": "forged" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!(40000));

    let unconfigured = build_test_app(None, 1).await;
    let (status, body) = post(&unconfigured, "subnets", json!({ "token": "anything" })).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], json!(50300));
}

#[tokio::test]
async fn external_options_maps_json_rejections_to_feishu_contract() {
    let app = build_test_app(Some("approval-secret"), 1).await;
    let (status, body) = post_raw(&app, "subnets", "application/json", "{not-json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!(40001));
    assert_eq!(body["data"], Value::Null);

    let (status, body) = post_raw(&app, "subnets", "text/plain", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!(40001));
}
