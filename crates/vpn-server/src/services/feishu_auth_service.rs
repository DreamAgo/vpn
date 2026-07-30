//! 飞书 OAuth：固定服务端回调 + 桌面客户端一次性短轮询。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration as StdDuration,
};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;
use vpn_api_types::auth::{
    FeishuAuthPollResponse, FeishuAuthPollStatus, FeishuAuthStartResponse, LoginResponse,
};
use vpn_core::{AppError, Result};

use crate::{
    config::FeishuConfig,
    repositories::SqliteUserRepository,
    services::{AuthService, LoginOutcome},
};

const FLOW_TTL_SECS: i64 = 180;
const MAX_ACTIVE_FLOWS: usize = 2_048;
const START_RATE_WINDOW_SECS: i64 = 60;
const MAX_STARTS_PER_WINDOW: usize = 10;
const PROVIDER: &str = "feishu";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuIdentity {
    pub subject: String,
    pub email: String,
}

#[async_trait]
pub trait FeishuIdentityProvider: Send + Sync {
    async fn exchange_identity(&self, code: &str) -> Result<FeishuIdentity>;
}

#[derive(Clone)]
pub struct ReqwestFeishuIdentityProvider {
    http: reqwest::Client,
    config: FeishuConfig,
}

impl ReqwestFeishuIdentityProvider {
    pub fn new(config: FeishuConfig) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(StdDuration::from_secs(5))
                .timeout(StdDuration::from_secs(15))
                .build()
                .map_err(internal)?,
            config,
        })
    }
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    client_secret: &'a str,
    code: &'a str,
    redirect_uri: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct UserInfoEnvelope {
    code: i64,
    msg: String,
    data: Option<UserInfo>,
}

#[derive(Deserialize)]
struct UserInfo {
    union_id: Option<String>,
    email: Option<String>,
    enterprise_email: Option<String>,
}

#[async_trait]
impl FeishuIdentityProvider for ReqwestFeishuIdentityProvider {
    async fn exchange_identity(&self, code: &str) -> Result<FeishuIdentity> {
        let app_id = self.config.app_id.as_deref().ok_or_else(disabled)?;
        let app_secret = self.config.app_secret.as_deref().ok_or_else(disabled)?;
        let redirect_uri = self.config.redirect_uri.as_deref().ok_or_else(disabled)?;
        let token = self
            .http
            .post("https://open.feishu.cn/open-apis/authen/v2/oauth/token")
            .json(&TokenRequest {
                grant_type: "authorization_code",
                client_id: app_id,
                client_secret: app_secret,
                code,
                redirect_uri,
            })
            .send()
            .await
            .map_err(internal)?
            .error_for_status()
            .map_err(internal)?
            .json::<TokenResponse>()
            .await
            .map_err(internal)?;
        let info = self
            .http
            .get("https://open.feishu.cn/open-apis/authen/v1/user_info")
            .bearer_auth(token.access_token)
            .send()
            .await
            .map_err(internal)?
            .error_for_status()
            .map_err(internal)?
            .json::<UserInfoEnvelope>()
            .await
            .map_err(internal)?;
        if info.code != 0 {
            return Err(AppError::Validation(format!(
                "飞书用户信息请求失败：{}",
                info.msg
            )));
        }
        let info = info
            .data
            .ok_or_else(|| AppError::Validation("飞书未返回用户信息".into()))?;
        normalize_identity(info)
    }
}

fn normalize_identity(info: UserInfo) -> Result<FeishuIdentity> {
    let subject = info
        .union_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation("飞书账号缺少稳定 union_id".into()))?;
    let email = info
        .enterprise_email
        .filter(|v| !v.trim().is_empty())
        .or_else(|| info.email.filter(|v| !v.trim().is_empty()))
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| valid_email(value))
        .ok_or_else(|| AppError::Validation("飞书账号未提供有效邮箱".into()))?;
    Ok(FeishuIdentity { subject, email })
}

fn internal(error: reqwest::Error) -> AppError {
    AppError::Internal(Box::new(error))
}

fn disabled() -> AppError {
    AppError::Config("飞书登录未配置".to_string())
}

enum PollState {
    Pending,
    Verified(FeishuIdentity),
    Processing,
    Rejected(String),
}

struct Flow {
    expires_at: i64,
    state: PollState,
}

#[derive(Default)]
struct Flows {
    states: HashMap<String, String>,
    polls: HashMap<String, Flow>,
    starts: HashMap<String, VecDeque<i64>>,
}

#[derive(Clone)]
pub struct FeishuAuthService {
    config: FeishuConfig,
    provider: Arc<dyn FeishuIdentityProvider>,
    flows: Arc<Mutex<Flows>>,
    user_repo: SqliteUserRepository,
    auth_service: Arc<AuthService>,
}

impl FeishuAuthService {
    pub fn new(
        config: FeishuConfig,
        provider: Arc<dyn FeishuIdentityProvider>,
        user_repo: SqliteUserRepository,
        auth_service: Arc<AuthService>,
    ) -> Self {
        let service = Self {
            config,
            provider,
            flows: Arc::new(Mutex::new(Flows::default())),
            user_repo,
            auth_service,
        };
        service.spawn_expiry_cleanup();
        service
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    pub async fn start(&self, client_key: &str) -> Result<FeishuAuthStartResponse> {
        if !self.enabled() {
            return Err(disabled());
        }
        let state = random_token();
        let poll_token = random_token();
        let poll_hash = token_hash(&poll_token);
        let expires_at = Utc::now().timestamp() + FLOW_TTL_SECS;
        let mut flows = self.flows.lock().await;
        let now = Utc::now().timestamp();
        cleanup_flows(&mut flows, now);
        if flows.polls.len() >= MAX_ACTIVE_FLOWS {
            return Err(AppError::RateLimited);
        }
        let attempts = flows.starts.entry(client_key.to_string()).or_default();
        attempts.retain(|started_at| *started_at > now - START_RATE_WINDOW_SECS);
        if attempts.len() >= MAX_STARTS_PER_WINDOW {
            return Err(AppError::RateLimited);
        }
        attempts.push_back(now);
        flows.states.insert(state.clone(), poll_hash.clone());
        flows.polls.insert(
            poll_hash,
            Flow {
                expires_at,
                state: PollState::Pending,
            },
        );
        drop(flows);

        let mut url =
            reqwest::Url::parse("https://accounts.feishu.cn/open-apis/authen/v1/authorize")
                .map_err(|e| AppError::Internal(Box::new(e)))?;
        url.query_pairs_mut()
            .append_pair(
                "app_id",
                self.config.app_id.as_deref().ok_or_else(disabled)?,
            )
            .append_pair(
                "redirect_uri",
                self.config.redirect_uri.as_deref().ok_or_else(disabled)?,
            )
            .append_pair(
                "scope",
                "contact:user.base:readonly contact:user.email:readonly",
            )
            .append_pair("state", &state);
        Ok(FeishuAuthStartResponse {
            authorization_url: url.into(),
            poll_token,
            expires_in: FLOW_TTL_SECS,
        })
    }

    fn spawn_expiry_cleanup(&self) {
        let weak = Arc::downgrade(&self.flows);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut ticker = tokio::time::interval(StdDuration::from_secs(30));
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    let Some(flows) = weak.upgrade() else { break };
                    let mut guard = flows.lock().await;
                    cleanup_flows(&mut guard, Utc::now().timestamp());
                }
            });
        }
    }

    /// 消费 state；同一回调（包括授权码）只能交换一次。
    pub async fn callback(
        &self,
        state: &str,
        code: Option<&str>,
        oauth_error: Option<&str>,
    ) -> Result<()> {
        let poll_hash = {
            let mut flows = self.flows.lock().await;
            cleanup_flows(&mut flows, Utc::now().timestamp());
            let poll_hash = flows
                .states
                .remove(state)
                .ok_or_else(|| AppError::Validation("飞书登录 state 无效或已使用".into()))?;
            if !flows.polls.contains_key(&poll_hash) {
                return Err(AppError::Validation("飞书登录已过期".into()));
            }
            poll_hash
        };

        let result = if let Some(error) = oauth_error {
            Err(AppError::Validation(if error == "access_denied" {
                "用户取消了飞书授权".into()
            } else {
                "飞书授权失败".into()
            }))
        } else if let Some(code) = code {
            self.provider.exchange_identity(code).await
        } else {
            Err(AppError::Validation("飞书回调缺少授权码".into()))
        };
        let mut flows = self.flows.lock().await;
        if let Some(flow) = flows.polls.get_mut(&poll_hash) {
            flow.state = match &result {
                Ok(identity) => PollState::Verified(identity.clone()),
                Err(error) => PollState::Rejected(error.to_string()),
            };
        }
        result.map(|_| ())
    }

    /// pending 可重复查看；完成、失败、过期均原子消费 poll token。
    pub async fn poll(
        &self,
        poll_token: &str,
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> Result<FeishuAuthPollResponse> {
        let poll_hash = token_hash(poll_token);
        let identity = {
            let mut flows = self.flows.lock().await;
            cleanup_flows(&mut flows, Utc::now().timestamp());
            let flow = flows
                .polls
                .get_mut(&poll_hash)
                .ok_or_else(|| AppError::Validation("飞书登录领取凭证无效或已使用".into()))?;
            match &flow.state {
                PollState::Pending => {
                    return Ok(FeishuAuthPollResponse {
                        status: FeishuAuthPollStatus::Pending,
                        username: None,
                        login: None,
                    })
                }
                PollState::Processing => {
                    return Ok(FeishuAuthPollResponse {
                        status: FeishuAuthPollStatus::Pending,
                        username: None,
                        login: None,
                    })
                }
                PollState::Rejected(reason) => {
                    let reason = reason.clone();
                    flows.polls.remove(&poll_hash);
                    return Err(AppError::Validation(reason));
                }
                PollState::Verified(identity) => {
                    let identity = identity.clone();
                    flow.state = PollState::Processing;
                    identity
                }
            }
        };
        let outcome = match self.login_identity(&identity, ip, ua).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let mut flows = self.flows.lock().await;
                if let Some(flow) = flows.polls.get_mut(&poll_hash) {
                    if matches!(flow.state, PollState::Processing) {
                        flow.state = PollState::Verified(identity);
                    }
                }
                return Err(error);
            }
        };
        self.flows.lock().await.polls.remove(&poll_hash);
        let username = outcome.user.username.clone();
        Ok(FeishuAuthPollResponse {
            status: FeishuAuthPollStatus::Complete,
            username: Some(username),
            login: Some(login_response(outcome)),
        })
    }

    async fn login_identity(
        &self,
        identity: &FeishuIdentity,
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> Result<LoginOutcome> {
        if let Some(user) = self
            .user_repo
            .find_by_external_identity(PROVIDER, &identity.subject)
            .await?
        {
            return self.auth_service.login_user(user, ip, ua).await;
        }
        let local = identity.email.split('@').next().unwrap_or("feishu");
        let preferred: String = local
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .collect();
        let preferred = if preferred.is_empty() {
            "feishu"
        } else {
            &preferred
        };
        let suffix = &token_hash(&identity.subject)[..8];
        let random_password = random_token();
        let password_hash = self.auth_service.hasher.hash(&random_password)?;
        let user = self
            .user_repo
            .resolve_external_identity(
                PROVIDER,
                &identity.subject,
                &identity.email,
                preferred,
                suffix,
                &Uuid::now_v7().to_string(),
                &password_hash,
            )
            .await?;
        if user.status == "disabled" {
            return Err(AppError::AccountDisabled);
        }
        self.auth_service.login_user(user, ip, ua).await
    }
}

fn cleanup_flows(flows: &mut Flows, now: i64) {
    flows.polls.retain(|_, flow| flow.expires_at > now);
    let active_polls: HashSet<String> = flows.polls.keys().cloned().collect();
    flows
        .states
        .retain(|_, poll_hash| active_polls.contains(poll_hash));
    for attempts in flows.starts.values_mut() {
        attempts.retain(|started_at| *started_at > now - START_RATE_WINDOW_SECS);
    }
    flows.starts.retain(|_, attempts| !attempts.is_empty());
}

fn valid_email(email: &str) -> bool {
    if email.is_empty() || email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    !local.is_empty() && domain.contains('.') && parts.next().is_none()
}

fn login_response(outcome: LoginOutcome) -> LoginResponse {
    LoginResponse {
        access_token: outcome.access_token,
        refresh_token: outcome.refresh_token,
        access_expires_in: AuthService::access_ttl_secs(),
        must_change_password: outcome.user.must_change_password,
    }
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ratelimit::LoginAttempts, repositories::SqliteSessionRepository};
    use sqlx::{
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
        SqlitePool,
    };
    use std::{
        str::FromStr,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use vpn_core::service::{PasswordHasher, TokenIssuer};

    struct FakeProvider {
        calls: AtomicUsize,
        identity: FeishuIdentity,
    }

    #[async_trait]
    impl FeishuIdentityProvider for FakeProvider {
        async fn exchange_identity(&self, _: &str) -> Result<FeishuIdentity> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.identity.clone())
        }
    }

    #[derive(Debug)]
    struct FakeHasher;
    impl PasswordHasher for FakeHasher {
        fn hash(&self, plaintext: &str) -> Result<String> {
            Ok(format!("hash:{plaintext}"))
        }
        fn verify(&self, _: &str, _: &str) -> Result<bool> {
            Ok(true)
        }
    }

    #[derive(Debug)]
    struct RejectingHasher;
    impl PasswordHasher for RejectingHasher {
        fn hash(&self, _: &str) -> Result<String> {
            Err(AppError::Internal(Box::new(std::io::Error::other(
                "hash must not be called for a bound identity",
            ))))
        }
        fn verify(&self, _: &str, _: &str) -> Result<bool> {
            Ok(false)
        }
    }

    #[derive(Debug)]
    struct FakeIssuer {
        access_failures: AtomicUsize,
    }
    #[async_trait]
    impl TokenIssuer for FakeIssuer {
        async fn issue_access(&self, user_id: &str, _: &str) -> Result<String> {
            if self
                .access_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                    value.checked_sub(1)
                })
                .is_ok()
            {
                return Err(AppError::Internal(Box::new(std::io::Error::other(
                    "temporary issuer failure",
                ))));
            }
            Ok(format!("access-{user_id}"))
        }
        async fn issue_refresh(&self, user_id: &str) -> Result<String> {
            Ok(format!("refresh-{user_id}-{}", Uuid::new_v4()))
        }
        async fn verify_access(&self, _: &str) -> Result<(String, String)> {
            unreachable!()
        }
        async fn verify_refresh(&self, _: &str) -> Result<String> {
            unreachable!()
        }
    }

    #[derive(Debug)]
    struct SlowIssuer {
        calls: AtomicUsize,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl TokenIssuer for SlowIssuer {
        async fn issue_access(&self, user_id: &str, _: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(format!("access-{user_id}"))
        }
        async fn issue_refresh(&self, user_id: &str) -> Result<String> {
            Ok(format!("refresh-{user_id}"))
        }
        async fn verify_access(&self, _: &str) -> Result<(String, String)> {
            unreachable!()
        }
        async fn verify_refresh(&self, _: &str) -> Result<String> {
            unreachable!()
        }
    }

    async fn setup(identity: FeishuIdentity) -> (FeishuAuthService, Arc<FakeProvider>, SqlitePool) {
        setup_with_failures(identity, 0).await
    }

    async fn setup_with_failures(
        identity: FeishuIdentity,
        access_failures: usize,
    ) -> (FeishuAuthService, Arc<FakeProvider>, SqlitePool) {
        let url = format!(
            "sqlite:file:feishu_test_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let users = SqliteUserRepository::new(pool.clone());
        let auth = Arc::new(AuthService {
            user_repo: users.clone(),
            session_repo: SqliteSessionRepository::new(pool.clone()),
            hasher: Arc::new(FakeHasher),
            issuer: Arc::new(FakeIssuer {
                access_failures: AtomicUsize::new(access_failures),
            }),
            login_attempts: LoginAttempts::new(),
        });
        let provider = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            identity,
        });
        let service = FeishuAuthService::new(
            FeishuConfig {
                app_id: Some("app-id".into()),
                app_secret: Some("secret".into()),
                redirect_uri: Some("https://vpn.example.com/api/v1/auth/feishu/callback".into()),
            },
            provider.clone(),
            users,
            auth,
        );
        (service, provider, pool)
    }

    fn state_from_url(url: &str) -> String {
        reqwest::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned()
    }

    #[tokio::test]
    async fn flow_creates_user_and_rejects_callback_and_poll_replays() {
        let (service, provider, pool) = setup(FeishuIdentity {
            subject: "union-1".into(),
            email: "alice@example.com".into(),
        })
        .await;
        let started = service.start("127.0.0.1").await.unwrap();
        assert_eq!(
            service
                .poll(&started.poll_token, None, None)
                .await
                .unwrap()
                .status,
            FeishuAuthPollStatus::Pending
        );
        let state = state_from_url(&started.authorization_url);
        service
            .callback(&state, Some("one-time-code"), None)
            .await
            .unwrap();
        assert!(service
            .callback(&state, Some("one-time-code"), None)
            .await
            .is_err());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let complete = service.poll(&started.poll_token, None, None).await.unwrap();
        assert_eq!(complete.status, FeishuAuthPollStatus::Complete);
        assert!(complete
            .login
            .unwrap()
            .refresh_token
            .starts_with("refresh-"));
        assert!(service.poll(&started.poll_token, None, None).await.is_err());
        let user: (String, i64, i64) = sqlx::query_as("SELECT role, max_devices, must_change_password FROM users WHERE email = 'alice@example.com'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(user, ("user".into(), 1, 0));
    }

    #[tokio::test]
    async fn matching_email_binds_existing_user_and_disabled_user_is_rejected() {
        let identity = FeishuIdentity {
            subject: "union-2".into(),
            email: "alice@example.com".into(),
        };
        let (service, _, pool) = setup(identity).await;
        sqlx::query("INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, max_devices, created_at, updated_at) VALUES ('existing', 'alice', 'alice@example.com', 'h', 'user', 'active', 0, 1, 0, 0)")
            .execute(&pool).await.unwrap();
        let started = service.start("127.0.0.1").await.unwrap();
        service
            .callback(
                &state_from_url(&started.authorization_url),
                Some("code"),
                None,
            )
            .await
            .unwrap();
        service.poll(&started.poll_token, None, None).await.unwrap();
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
        sqlx::query("UPDATE users SET status = 'disabled' WHERE id = 'existing'")
            .execute(&pool)
            .await
            .unwrap();

        let started = service.start("127.0.0.1").await.unwrap();
        service
            .callback(
                &state_from_url(&started.authorization_url),
                Some("code2"),
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            service.poll(&started.poll_token, None, None).await,
            Err(AppError::AccountDisabled)
        ));
    }

    #[tokio::test]
    async fn verified_flow_is_retryable_after_session_failure() {
        let (service, _, _) = setup_with_failures(
            FeishuIdentity {
                subject: "union-retry".into(),
                email: "retry@example.com".into(),
            },
            1,
        )
        .await;
        let started = service.start("retry-client").await.unwrap();
        service
            .callback(
                &state_from_url(&started.authorization_url),
                Some("code"),
                None,
            )
            .await
            .unwrap();
        assert!(service.poll(&started.poll_token, None, None).await.is_err());
        let completed = service.poll(&started.poll_token, None, None).await.unwrap();
        assert_eq!(completed.status, FeishuAuthPollStatus::Complete);
        assert_eq!(completed.username.as_deref(), Some("retry"));
    }

    #[tokio::test]
    async fn concurrent_poll_has_only_one_session_processor() {
        let (_, _, pool) = setup(FeishuIdentity {
            subject: "unused".into(),
            email: "unused@example.com".into(),
        })
        .await;
        let users = SqliteUserRepository::new(pool.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let issuer = Arc::new(SlowIssuer {
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        });
        let auth = Arc::new(AuthService {
            user_repo: users.clone(),
            session_repo: SqliteSessionRepository::new(pool),
            hasher: Arc::new(FakeHasher),
            issuer: issuer.clone(),
            login_attempts: LoginAttempts::new(),
        });
        let service = FeishuAuthService::new(
            FeishuConfig {
                app_id: Some("app-id".into()),
                app_secret: Some("secret".into()),
                redirect_uri: Some("https://vpn.example.com/api/v1/auth/feishu/callback".into()),
            },
            Arc::new(FakeProvider {
                calls: AtomicUsize::new(0),
                identity: FeishuIdentity {
                    subject: "union-concurrent".into(),
                    email: "concurrent@example.com".into(),
                },
            }),
            users,
            auth,
        );
        let started = service.start("concurrent-client").await.unwrap();
        service
            .callback(
                &state_from_url(&started.authorization_url),
                Some("code"),
                None,
            )
            .await
            .unwrap();
        let first_service = service.clone();
        let first_token = started.poll_token.clone();
        let first = tokio::spawn(async move { first_service.poll(&first_token, None, None).await });
        entered.notified().await;
        let second = service.poll(&started.poll_token, None, None).await.unwrap();
        assert_eq!(second.status, FeishuAuthPollStatus::Pending);
        release.notify_one();
        assert_eq!(
            first.await.unwrap().unwrap().status,
            FeishuAuthPollStatus::Complete
        );
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn start_is_rate_limited_and_flow_capacity_is_bounded() {
        let (service, _, _) = setup(FeishuIdentity {
            subject: "union-limit".into(),
            email: "limit@example.com".into(),
        })
        .await;
        for _ in 0..MAX_STARTS_PER_WINDOW {
            service.start("same-client").await.unwrap();
        }
        assert!(matches!(
            service.start("same-client").await,
            Err(AppError::RateLimited)
        ));

        for index in MAX_STARTS_PER_WINDOW..MAX_ACTIVE_FLOWS {
            service.start(&format!("client-{index}")).await.unwrap();
        }
        assert!(matches!(
            service.start("overflow-client").await,
            Err(AppError::RateLimited)
        ));
        assert_eq!(service.flows.lock().await.polls.len(), MAX_ACTIVE_FLOWS);
    }

    #[tokio::test]
    async fn cleanup_removes_expired_identity_and_state() {
        let (service, _, _) = setup(FeishuIdentity {
            subject: "union-clean".into(),
            email: "clean@example.com".into(),
        })
        .await;
        let mut flows = service.flows.lock().await;
        flows.states.insert("state".into(), "poll".into());
        flows.polls.insert(
            "poll".into(),
            Flow {
                expires_at: Utc::now().timestamp() - 1,
                state: PollState::Verified(FeishuIdentity {
                    subject: "secret".into(),
                    email: "secret@example.com".into(),
                }),
            },
        );
        cleanup_flows(&mut flows, Utc::now().timestamp());
        assert!(flows.states.is_empty());
        assert!(flows.polls.is_empty());
    }

    #[tokio::test]
    async fn bound_identity_login_skips_password_hashing() {
        let (_, _, pool) = setup(FeishuIdentity {
            subject: "unused".into(),
            email: "unused@example.com".into(),
        })
        .await;
        let users = SqliteUserRepository::new(pool.clone());
        users
            .resolve_external_identity(
                PROVIDER,
                "union-bound",
                "bound@example.com",
                "bound",
                "abcd1234",
                "bound-user",
                "existing-hash",
            )
            .await
            .unwrap();
        let auth = Arc::new(AuthService {
            user_repo: users.clone(),
            session_repo: SqliteSessionRepository::new(pool),
            hasher: Arc::new(RejectingHasher),
            issuer: Arc::new(FakeIssuer {
                access_failures: AtomicUsize::new(0),
            }),
            login_attempts: LoginAttempts::new(),
        });
        let provider = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            identity: FeishuIdentity {
                subject: "union-bound".into(),
                email: "changed@example.com".into(),
            },
        });
        let service = FeishuAuthService::new(
            FeishuConfig {
                app_id: Some("app-id".into()),
                app_secret: Some("secret".into()),
                redirect_uri: Some("https://vpn.example.com/api/v1/auth/feishu/callback".into()),
            },
            provider,
            users,
            auth,
        );
        let started = service.start("bound-client").await.unwrap();
        service
            .callback(
                &state_from_url(&started.authorization_url),
                Some("code"),
                None,
            )
            .await
            .unwrap();
        let completed = service.poll(&started.poll_token, None, None).await.unwrap();
        assert_eq!(completed.username.as_deref(), Some("bound"));
    }

    #[test]
    fn provider_identity_requires_union_id_and_normalizes_email() {
        let identity = normalize_identity(UserInfo {
            union_id: Some(" union-1 ".into()),
            email: Some(" Alice@Example.COM ".into()),
            enterprise_email: None,
        })
        .unwrap();
        assert_eq!(identity.subject, "union-1");
        assert_eq!(identity.email, "alice@example.com");
        assert!(normalize_identity(UserInfo {
            union_id: Some(" ".into()),
            email: Some("a@example.com".into()),
            enterprise_email: None,
        })
        .is_err());
        assert!(normalize_identity(UserInfo {
            union_id: Some("union".into()),
            email: Some("invalid".into()),
            enterprise_email: None,
        })
        .is_err());
    }
}
