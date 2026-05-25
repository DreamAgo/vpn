//! Story 4.12: HTTP API 客户端。
//!
//! 用 reqwest 封装服务端 REST API：
//! - `login` / `refresh` / `logout`
//! - `register_peer` / `heartbeat`
//! - `download_config`
//!
//! 统一处理 [`ApiResponse`] 信封解包（code==0 取 data，否则映射为
//! [`CliError::Api`]），access token 过期（code 1002 / HTTP 401）时自动用
//! refresh token 刷新一次并重试请求。
//!
//! 设计上把「信封解包」这一纯逻辑（[`unwrap_envelope`]）从 IO 中剥离，便于单测；
//! 网络层用 `wiremock` 做端到端验证（见 tests/）。

use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;
use vpn_api_types::{
    auth::{LoginRequest, LoginResponse, LogoutRequest, RefreshRequest, RefreshResponse},
    peer::{PeerHeartbeatRequest, PeerRegisterRequest, PeerRegisterResponse},
    ApiResponse,
};

use crate::error::{CliError, CliResult};

/// 纯逻辑：把已反序列化的 [`ApiResponse`] 信封解包为 `data`。
///
/// - `code == 0`：返回 `data`（若 `data` 为 `None` 视为解码错误）。
/// - `code != 0`：映射为 [`CliError::Api`]。
///
/// 该函数不涉及 IO，便于单测错误码映射与成功取值逻辑。
pub fn unwrap_envelope<T>(resp: ApiResponse<T>) -> CliResult<T> {
    if resp.code == 0 {
        resp.data
            .ok_or_else(|| CliError::Decode("成功响应缺少 data 字段".to_string()))
    } else {
        Err(CliError::Api {
            code: resp.code,
            message: resp.message,
        })
    }
}

/// 纯逻辑：把响应文本解析为信封并解包。便于单测「无网络」路径。
///
/// 注意：服务端对「无返回数据」的成功响应（如 logout / heartbeat）会发送
/// `data: null`。对此类响应，若 `code == 0` 且 `data` 为 `None`，则尝试从
/// `null` 反序列化目标类型 `T`（对 `()` / `Option<_>` 成立），从而正确解包
/// 单元类型的成功响应。
pub fn parse_and_unwrap<T: DeserializeOwned>(body: &str) -> CliResult<T> {
    let envelope: ApiResponse<T> = serde_json::from_str(body)?;
    if envelope.code == 0 && envelope.data.is_none() {
        // 成功但无 data：对单元类型从 null 恢复。
        return serde_json::from_value::<T>(serde_json::Value::Null).map_err(|_| {
            CliError::Decode("成功响应缺少 data 字段，且目标类型非单元类型".to_string())
        });
    }
    unwrap_envelope(envelope)
}

/// 已认证会话的 token 状态。
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    /// 当前 access token（Bearer）。
    pub access_token: Option<String>,
    /// refresh token（用于刷新 access token）。
    pub refresh_token: Option<String>,
}

/// HTTP API 客户端。
pub struct ApiClient {
    base_url: String,
    http: reqwest::Client,
    tokens: Mutex<Tokens>,
}

impl ApiClient {
    /// 创建客户端。`base_url` 形如 `https://vpn.example.com`（不含末尾 `/`）。
    pub fn new(base_url: impl Into<String>) -> CliResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vpn-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self::with_http(base_url, http))
    }

    /// 以给定 reqwest client 构造（便于注入超时配置 / 测试）。
    pub fn with_http(base_url: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            base_url: normalize_base_url(base_url.into()),
            http,
            tokens: Mutex::new(Tokens::default()),
        }
    }

    /// 设置已持有的 refresh token（例如从凭证存储恢复后）。
    pub fn set_refresh_token(&self, token: impl Into<String>) {
        self.tokens.lock().unwrap().refresh_token = Some(token.into());
    }

    /// 当前 refresh token（用于持久化）。
    pub fn refresh_token(&self) -> Option<String> {
        self.tokens.lock().unwrap().refresh_token.clone()
    }

    /// 当前 access token。
    pub fn access_token(&self) -> Option<String> {
        self.tokens.lock().unwrap().access_token.clone()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // === 认证 ===

    /// 登录：成功后内部保存 access + refresh token。
    pub async fn login(&self, username: &str, password: &str) -> CliResult<LoginResponse> {
        let req = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        };
        let resp: LoginResponse = self.post_json_unauthed("/api/v1/auth/login", &req).await?;
        {
            let mut t = self.tokens.lock().unwrap();
            t.access_token = Some(resp.access_token.clone());
            t.refresh_token = Some(resp.refresh_token.clone());
        }
        Ok(resp)
    }

    /// 用 refresh token 换取新的 access token，并更新内部状态。
    pub async fn refresh(&self) -> CliResult<RefreshResponse> {
        let refresh_token = self
            .tokens
            .lock()
            .unwrap()
            .refresh_token
            .clone()
            .ok_or(CliError::NotLoggedIn)?;
        let req = RefreshRequest { refresh_token };
        let resp: RefreshResponse = self
            .post_json_unauthed("/api/v1/auth/refresh", &req)
            .await?;
        self.tokens.lock().unwrap().access_token = Some(resp.access_token.clone());
        Ok(resp)
    }

    /// 注销当前 refresh token（服务端吊销）。
    pub async fn logout(&self) -> CliResult<()> {
        let refresh_token = self
            .tokens
            .lock()
            .unwrap()
            .refresh_token
            .clone()
            .ok_or(CliError::NotLoggedIn)?;
        let req = LogoutRequest { refresh_token };
        // logout 端点需要 Bearer（authed_routes）。
        let _: () = self.post_json_authed("/api/v1/auth/logout", &req).await?;
        let mut t = self.tokens.lock().unwrap();
        t.access_token = None;
        t.refresh_token = None;
        Ok(())
    }

    // === Peer 数据平面 ===

    /// 注册本设备，返回分配的 VPN IP + 服务端信息。
    pub async fn register_peer(
        &self,
        req: &PeerRegisterRequest,
    ) -> CliResult<PeerRegisterResponse> {
        self.post_json_authed("/api/v1/peers/register", req).await
    }

    /// 周期性心跳上报（每 30s）。
    pub async fn heartbeat(&self, req: &PeerHeartbeatRequest) -> CliResult<()> {
        self.post_json_authed("/api/v1/peers/heartbeat", req).await
    }

    /// 下载客户端 .conf 配置（非信封，纯文本附件）。
    pub async fn download_config(&self) -> CliResult<String> {
        let access = self.require_access()?;
        let do_req = |token: String| {
            self.http
                .get(self.url("/api/v1/peers/me/config"))
                .bearer_auth(token)
        };
        let resp = do_req(access).send().await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            // 刷新后重试一次。
            self.refresh().await?;
            let access = self.require_access()?;
            let resp = do_req(access).send().await?;
            return Ok(resp.error_for_status()?.text().await?);
        }
        Ok(resp.error_for_status()?.text().await?)
    }

    // === 内部 HTTP 辅助 ===

    fn require_access(&self) -> CliResult<String> {
        self.tokens
            .lock()
            .unwrap()
            .access_token
            .clone()
            .ok_or(CliError::NotLoggedIn)
    }

    /// 无需鉴权的 POST（login / refresh）。
    async fn post_json_unauthed<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        let resp = self.http.post(self.url(path)).json(body).send().await?;
        let text = resp.text().await?;
        parse_and_unwrap(&text)
    }

    /// 需 Bearer 鉴权的 POST，遇 token 过期自动刷新一次重试。
    async fn post_json_authed<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        match self.post_json_authed_once(path, body).await {
            Err(e) if e.is_token_expired() => {
                // 刷新 access token 后重试一次。
                self.refresh().await?;
                self.post_json_authed_once(path, body).await
            }
            other => other,
        }
    }

    async fn post_json_authed_once<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        let access = self.require_access()?;
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(access)
            .json(body)
            .send()
            .await?;
        let text = resp.text().await?;
        parse_and_unwrap(&text)
    }
}

/// 规整 base_url：去掉末尾的 `/`，避免拼接出 `//`。
fn normalize_base_url(mut url: String) -> String {
    while url.ends_with('/') {
        url.pop();
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_api_types::error_codes;

    fn envelope_json<T: Serialize>(code: i32, message: &str, data: Option<T>) -> String {
        let env = ApiResponse {
            code,
            message: message.to_string(),
            data,
            timestamp: 1,
            request_id: "req".to_string(),
        };
        serde_json::to_string(&env).unwrap()
    }

    #[test]
    fn unwrap_success_returns_data() {
        let env: ApiResponse<i32> = ApiResponse::success(42, "r".into(), 1);
        assert_eq!(unwrap_envelope(env).unwrap(), 42);
    }

    #[test]
    fn unwrap_error_maps_to_api_error() {
        let env: ApiResponse<i32> = ApiResponse::error(
            error_codes::INVALID_CREDENTIALS,
            "bad".into(),
            "r".into(),
            1,
        );
        let err = unwrap_envelope(env).unwrap_err();
        match err {
            CliError::Api { code, message } => {
                assert_eq!(code, error_codes::INVALID_CREDENTIALS);
                assert_eq!(message, "bad");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn unwrap_success_missing_data_is_decode_error() {
        let env: ApiResponse<i32> = ApiResponse {
            code: 0,
            message: "success".into(),
            data: None,
            timestamp: 1,
            request_id: "r".into(),
        };
        assert!(matches!(unwrap_envelope(env), Err(CliError::Decode(_))));
    }

    #[test]
    fn parse_and_unwrap_login_response() {
        let body = envelope_json(
            0,
            "success",
            Some(LoginResponse {
                access_token: "atk".into(),
                refresh_token: "rtk".into(),
                access_expires_in: 900,
                must_change_password: false,
            }),
        );
        let resp: LoginResponse = parse_and_unwrap(&body).unwrap();
        assert_eq!(resp.access_token, "atk");
        assert_eq!(resp.refresh_token, "rtk");
    }

    #[test]
    fn parse_and_unwrap_token_expired() {
        let body = envelope_json::<()>(error_codes::TOKEN_EXPIRED, "token 过期", None);
        let err = parse_and_unwrap::<()>(&body).unwrap_err();
        assert!(err.is_token_expired());
    }

    #[test]
    fn base_url_normalization() {
        assert_eq!(normalize_base_url("https://x.com/".into()), "https://x.com");
        assert_eq!(
            normalize_base_url("https://x.com///".into()),
            "https://x.com"
        );
        assert_eq!(normalize_base_url("https://x.com".into()), "https://x.com");
    }

    #[test]
    fn client_tracks_refresh_token() {
        let c = ApiClient::new("https://x.com").unwrap();
        assert_eq!(c.refresh_token(), None);
        c.set_refresh_token("rtk");
        assert_eq!(c.refresh_token(), Some("rtk".to_string()));
    }
}
