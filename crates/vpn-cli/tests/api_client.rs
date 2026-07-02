//! Story 4.12: ApiClient 端到端测试（用 wiremock 模拟服务端，无真实网络）。
//!
//! 覆盖：登录取 token、注册 peer、心跳、token 过期自动刷新重试、错误码映射。

use vpn_api_types::{
    auth::{LoginResponse, RefreshResponse},
    peer::{PeerRegisterRequest, PeerRegisterResponse},
    ApiResponse,
};
use vpn_cli::api::ApiClient;
use vpn_cli::error::CliError;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope<T: serde::Serialize>(data: T) -> serde_json::Value {
    serde_json::to_value(ApiResponse::success(data, "req".to_string(), 1)).unwrap()
}

fn err_envelope(code: i32, message: &str) -> serde_json::Value {
    serde_json::to_value(ApiResponse::<()>::error(
        code,
        message.to_string(),
        "req".to_string(),
        1,
    ))
    .unwrap()
}

#[tokio::test]
async fn login_stores_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(LoginResponse {
                access_token: "atk".into(),
                refresh_token: "rtk".into(),
                access_expires_in: 900,
                must_change_password: false,
            })),
        )
        .mount(&server)
        .await;

    let client = ApiClient::new(server.uri()).unwrap();
    let resp = client.login("alice", "pw").await.unwrap();
    assert_eq!(resp.access_token, "atk");
    assert_eq!(client.access_token(), Some("atk".to_string()));
    assert_eq!(client.refresh_token(), Some("rtk".to_string()));
}

#[tokio::test]
async fn login_bad_credentials_maps_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(err_envelope(1001, "用户名或密码错误")),
        )
        .mount(&server)
        .await;

    let client = ApiClient::new(server.uri()).unwrap();
    let err = client.login("alice", "wrong").await.unwrap_err();
    match err {
        CliError::Api { code, .. } => assert_eq!(code, 1001),
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn register_peer_succeeds_with_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/peers/register"))
        .and(header("authorization", "Bearer atk"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(PeerRegisterResponse {
                vpn_ip: "10.8.0.5".into(),
                server_public_key: "spk".into(),
                server_endpoint: "vpn.example.com:51820".into(),
                vpn_subnet: "10.8.0.0/24".into(),
                allowed_routes: vec!["10.8.0.0/24".into()],
            })),
        )
        .mount(&server)
        .await;

    let client = ApiClient::new(server.uri()).unwrap();
    client.set_refresh_token("rtk");
    // 先模拟拿到 access token：直接走 login mock 太重，这里手动设置。
    // ApiClient 没有公开 set_access；用一次 refresh 注入。
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(RefreshResponse {
                access_token: "atk".into(),
                access_expires_in: 900,
            })),
        )
        .mount(&server)
        .await;
    client.refresh().await.unwrap();

    let req = PeerRegisterRequest {
        wg_public_key: "pk".into(),
        device_name: "dev".into(),
        os_info: None,
        routed_subnets: Vec::new(),
        client_version: None,
    };
    let resp = client.register_peer(&req).await.unwrap();
    assert_eq!(resp.vpn_ip, "10.8.0.5");
    assert_eq!(resp.vpn_subnet, "10.8.0.0/24");
}

#[tokio::test]
async fn expired_access_token_triggers_refresh_and_retry() {
    let server = MockServer::start().await;

    // 第一次 register 用过期 token -> 返回 401 + code 1002。
    Mock::given(method("POST"))
        .and(path("/api/v1/peers/register"))
        .and(header("authorization", "Bearer stale"))
        .respond_with(ResponseTemplate::new(401).set_body_json(err_envelope(1002, "token 过期")))
        .mount(&server)
        .await;

    // refresh 换新 token。
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(RefreshResponse {
                access_token: "fresh".into(),
                access_expires_in: 900,
            })),
        )
        .mount(&server)
        .await;

    // 用新 token 的 register 成功。
    Mock::given(method("POST"))
        .and(path("/api/v1/peers/register"))
        .and(header("authorization", "Bearer fresh"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(PeerRegisterResponse {
                vpn_ip: "10.8.0.9".into(),
                server_public_key: "spk".into(),
                server_endpoint: "ep:51820".into(),
                vpn_subnet: "10.8.0.0/24".into(),
                allowed_routes: vec!["10.8.0.0/24".into()],
            })),
        )
        .mount(&server)
        .await;

    let client = ApiClient::new(server.uri()).unwrap();
    client.set_refresh_token("rtk");
    // 注入一个过期 access token：通过 refresh 拿到 "fresh" 会冲突，故手动构造场景——
    // 先 set_refresh，再人为放入 stale access：借助一次失败的 register 触发 refresh。
    // ApiClient 暴露不了 set_access，这里用 login mock 设 stale。
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(LoginResponse {
                access_token: "stale".into(),
                refresh_token: "rtk".into(),
                access_expires_in: 900,
                must_change_password: false,
            })),
        )
        .mount(&server)
        .await;
    client.login("alice", "pw").await.unwrap();
    assert_eq!(client.access_token(), Some("stale".to_string()));

    let req = PeerRegisterRequest {
        wg_public_key: "pk".into(),
        device_name: "dev".into(),
        os_info: None,
        routed_subnets: Vec::new(),
        client_version: None,
    };
    let resp = client.register_peer(&req).await.unwrap();
    assert_eq!(resp.vpn_ip, "10.8.0.9");
    // 重试后 access token 应已刷新。
    assert_eq!(client.access_token(), Some("fresh".to_string()));
}

#[tokio::test]
async fn heartbeat_posts_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/peers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            vpn_api_types::peer::PeerHeartbeatResponse {
                allowed_routes: vec!["10.8.0.0/24".into(), "172.31.100.0/24".into()],
            },
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(LoginResponse {
                access_token: "atk".into(),
                refresh_token: "rtk".into(),
                access_expires_in: 900,
                must_change_password: false,
            })),
        )
        .mount(&server)
        .await;

    let client = ApiClient::new(server.uri()).unwrap();
    client.login("a", "b").await.unwrap();
    let req = vpn_api_types::peer::PeerHeartbeatRequest {
        endpoint: Some("1.2.3.4:1234".into()),
        wg_public_key: None,
        rtt_ms: None,
        loss_pct: None,
    };
    let resp = client.heartbeat(&req).await.unwrap();
    assert_eq!(resp.allowed_routes, vec!["10.8.0.0/24", "172.31.100.0/24"]);
}
