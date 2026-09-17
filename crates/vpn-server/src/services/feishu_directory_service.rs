//! 飞书身份绑定、通讯录状态缓存与 durable 事件同步。
use super::feishu_approval_service::{
    decrypt_event, string_at, verify_signature, verify_timestamp, verify_token,
};
use super::{
    ApprovalEventHeaders, ApprovalWebhookReply, NetworkAclService, PeerService,
    ReqwestFeishuApprovalApi,
};
use crate::{
    config::{FeishuApprovalConfig, FeishuConfig},
    repositories::SqliteSessionRepository,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, OwnedRwLockWriteGuard, RwLock};
use vpn_api_types::user::{FeishuBindingDto, FeishuLookupRequest};
use vpn_core::{AppError, Result};

const SYNC_INTERVAL_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct DirectoryUser {
    pub union_id: String,
    pub open_id: Option<String>,
    pub user_id: Option<String>,
    pub name: String,
    pub email: String,
    pub status: String,
}
impl DirectoryUser {
    pub fn blocked(&self) -> bool {
        self.status != "active"
    }
    pub fn view(&self) -> FeishuBindingDto {
        FeishuBindingDto {
            union_id: self.union_id.clone(),
            name: self.name.clone(),
            email: self.email.clone(),
            status: self.status.clone(),
            blocked: self.blocked(),
            synced_at: Some(Utc::now().timestamp_millis()),
            last_error: None,
        }
    }
}

#[async_trait]
pub trait DirectoryApi: Send + Sync {
    async fn user(&self, id: &str, id_type: &str) -> Result<DirectoryUser>;
}

pub struct ReqwestDirectoryApi {
    tokens: ReqwestFeishuApprovalApi,
    http: reqwest::Client,
}
impl ReqwestDirectoryApi {
    pub fn new(config: FeishuConfig) -> Result<Self> {
        Ok(Self {
            tokens: ReqwestFeishuApprovalApi::new(config)?,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| AppError::Config("通讯录 HTTP 客户端初始化失败".into()))?,
        })
    }
}
#[async_trait]
impl DirectoryApi for ReqwestDirectoryApi {
    async fn user(&self, id: &str, id_type: &str) -> Result<DirectoryUser> {
        validate_lookup(id, id_type)?;
        let token = self.tokens.tenant_token().await?;
        let mut url = reqwest::Url::parse("https://open.feishu.cn/open-apis/contact/v3/users/")
            .expect("static URL");
        url.path_segments_mut()
            .expect("base URL")
            .pop_if_empty()
            .push(id);
        let response = self
            .http
            .get(url)
            .query(&[("user_id_type", id_type)])
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| AppError::Config("飞书通讯录请求失败，请稍后重试".into()))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|_| AppError::Config("飞书通讯录响应格式错误".into()))?;
        let code = value.get("code").and_then(Value::as_i64);
        if !status.is_success() || code != Some(0) {
            return Err(AppError::Config(format!(
                "飞书通讯录读取失败（HTTP {}，错误码 {}），请检查权限、通讯录范围与 IP 白名单",
                status.as_u16(),
                code.unwrap_or(-1)
            )));
        }
        parse_directory_user(
            value
                .pointer("/data/user")
                .ok_or_else(|| AppError::Config("飞书未返回用户信息".into()))?,
        )
    }
}
fn validate_lookup(id: &str, id_type: &str) -> Result<()> {
    if id.trim().is_empty()
        || id.len() > 256
        || !matches!(id_type, "open_id" | "user_id" | "union_id")
    {
        return Err(AppError::Validation("请填写有效飞书用户 ID 及类型".into()));
    }
    Ok(())
}
fn parse_directory_user(value: &Value) -> Result<DirectoryUser> {
    let status = value
        .get("status")
        .ok_or_else(|| AppError::Config("飞书未返回用户状态，请检查通讯录状态读取权限".into()))?;
    let flag = |key: &str| status.get(key).and_then(Value::as_bool);
    let state = if flag("is_resigned") == Some(true) {
        "resigned"
    } else if flag("is_exited") == Some(true) {
        "deleted"
    } else if flag("is_frozen") == Some(true) {
        "frozen"
    } else if flag("is_unjoin") == Some(true) || flag("is_activated") == Some(false) {
        "inactive"
    } else if flag("is_activated") == Some(true)
        && flag("is_frozen") == Some(false)
        && flag("is_resigned") == Some(false)
        && flag("is_exited") == Some(false)
    {
        "active"
    } else {
        return Err(AppError::Config(
            "飞书用户状态字段不完整，保留上次状态".into(),
        ));
    };
    Ok(DirectoryUser {
        union_id: string_at(value, &["union_id"])
            .filter(|v| !v.is_empty())
            .ok_or_else(|| AppError::Config("飞书用户缺少 union_id".into()))?,
        open_id: string_at(value, &["open_id"]),
        user_id: string_at(value, &["user_id"]),
        name: string_at(value, &["name"]).unwrap_or_default(),
        email: string_at(value, &["enterprise_email", "email"]).unwrap_or_default(),
        status: state.into(),
    })
}

#[derive(Clone)]
pub struct FeishuDirectoryService {
    pool: SqlitePool,
    app_id: String,
    config: FeishuApprovalConfig,
    api: Arc<dyn DirectoryApi>,
    peers: Option<Arc<PeerService>>,
    acl: Option<Arc<NetworkAclService>>,
    lock: Arc<Mutex<()>>,
    maintenance: Arc<RwLock<()>>,
}
impl FeishuDirectoryService {
    pub fn new(
        pool: SqlitePool,
        app_id: String,
        config: FeishuApprovalConfig,
        api: Arc<dyn DirectoryApi>,
        peers: Option<Arc<PeerService>>,
        acl: Option<Arc<NetworkAclService>>,
    ) -> Self {
        Self {
            pool,
            app_id,
            config,
            api,
            peers,
            acl,
            lock: Arc::new(Mutex::new(())),
            maintenance: Arc::new(RwLock::new(())),
        }
    }
    pub async fn pause_worker(&self) -> OwnedRwLockWriteGuard<()> {
        self.maintenance.clone().write_owned().await
    }
    pub async fn lookup(&self, req: &FeishuLookupRequest) -> Result<FeishuBindingDto> {
        validate_lookup(&req.user_id, &req.id_type)?;
        Ok(self
            .api
            .user(req.user_id.trim(), &req.id_type)
            .await?
            .view())
    }
    pub async fn bind(
        &self,
        user_id: &str,
        req: &FeishuLookupRequest,
    ) -> Result<Vec<FeishuBindingDto>> {
        let _maintenance = self.maintenance.read().await;
        let _lock = self.lock.lock().await;
        validate_lookup(&req.user_id, &req.id_type)?;
        // 从飞书重新取身份，不能信任浏览器提交的姓名/union_id 预览。
        let remote = self.api.user(req.user_id.trim(), &req.id_type).await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if !exists {
            return Err(AppError::UserNotFound);
        }
        let conflicts:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_identities WHERE provider='feishu' AND ((subject=?1 AND user_id!=?2) OR (user_id=?2 AND subject!=?1)))")
            .bind(&remote.union_id).bind(user_id).fetch_one(&mut *tx).await.map_err(db)?;
        if conflicts {
            return Err(AppError::DuplicateResource(
                "本地账号或飞书用户已绑定其他身份，不能覆盖绑定".into(),
            ));
        }
        sqlx::query("INSERT OR IGNORE INTO external_identities(provider,subject,user_id,created_at) VALUES('feishu',?1,?2,?3)")
            .bind(&remote.union_id).bind(user_id).bind(Utc::now().timestamp_millis()).execute(&mut *tx).await.map_err(db)?;
        // 绑定和状态同事务落库，冻结/离职账号不产生短暂可用窗口。
        persist_profile(&mut tx, &self.app_id, &remote, 0).await?;
        if remote.blocked() {
            sqlx::query(
                "UPDATE sessions SET revoked_at=?2 WHERE user_id=?1 AND revoked_at IS NULL",
            )
            .bind(user_id)
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        self.enforce().await?;
        bindings(&self.pool, user_id).await
    }
    pub async fn sync_user(&self, user_id: &str) -> Result<Vec<FeishuBindingDto>> {
        let _maintenance = self.maintenance.read().await;
        let _lock = self.lock.lock().await;
        let subjects: Vec<String> = sqlx::query_scalar(
            "SELECT subject FROM external_identities WHERE provider='feishu' AND user_id=?1",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        if subjects.is_empty() {
            return Err(AppError::Validation("用户尚未绑定飞书".into()));
        }
        for subject in subjects {
            self.sync_subject(&subject, 0).await?;
        }
        bindings(&self.pool, user_id).await
    }
    pub async fn queue_all(&self) -> Result<u64> {
        let _maintenance = self.maintenance.read().await;
        let _lock = self.lock.lock().await;
        sqlx::query("UPDATE feishu_user_states SET attempted_at=0 WHERE subject IN (SELECT subject FROM external_identities WHERE provider='feishu')")
            .execute(&self.pool).await.map_err(db)?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM external_identities WHERE provider='feishu'")
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        Ok(count as u64)
    }
    async fn sync_subject(&self, subject: &str, event_at: i64) -> Result<()> {
        let result = self.api.user(subject, "union_id").await;
        match result {
            Ok(user) if user.union_id == subject => {
                let mut tx = self.pool.begin().await.map_err(db)?;
                persist_profile(&mut tx, &self.app_id, &user, event_at).await?;
                tx.commit().await.map_err(db)?;
                self.enforce().await
            }
            other => {
                let error = match other {
                    Err(e) => e,
                    Ok(_) => AppError::Config("飞书返回身份与绑定不匹配".into()),
                };
                sqlx::query("INSERT INTO feishu_user_states(subject,app_id,attempted_at,last_error) VALUES(?1,?2,?3,?4) ON CONFLICT(subject) DO UPDATE SET attempted_at=excluded.attempted_at,last_error=excluded.last_error")
                    .bind(subject).bind(&self.app_id).bind(Utc::now().timestamp_millis()).bind(error.to_string()).execute(&self.pool).await.map_err(db)?;
                Err(error)
            }
        }
    }
    // 每次扫描都重试摘除，避免运行时失败后仅落数据库禁用而隧道仍保持。
    async fn enforce(&self) -> Result<()> {
        let users:Vec<String>=sqlx::query_scalar("SELECT DISTINCT e.user_id FROM external_identities e JOIN feishu_user_states f ON e.subject=f.subject WHERE e.provider='feishu' AND f.blocked=1")
            .fetch_all(&self.pool).await.map_err(db)?;
        let mut failure = None;
        for id in users {
            if let Err(error) = SqliteSessionRepository::new(self.pool.clone())
                .revoke_all_for_user(&id)
                .await
            {
                failure = Some(error);
            }
            if let Some(peers) = &self.peers {
                if let Err(error) = peers.force_remove_for_identity(&id).await {
                    failure = Some(error);
                }
            }
        }
        if let Some(acl) = &self.acl {
            if let Err(error) = acl.refresh().await {
                failure = Some(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                if let Err(error) = self.work_once().await {
                    tracing::warn!(error=%error,"飞书用户状态同步失败，将自动重试");
                }
            }
        });
    }
    pub async fn work_once(&self) -> Result<()> {
        let _maintenance = self.maintenance.read().await;
        let _lock = self.lock.lock().await;
        if let Err(error) = self.enforce().await {
            tracing::warn!(error=%error, "飞书用户访问限制执行失败，将重试；继续同步其他用户");
        }
        let now = Utc::now().timestamp_millis();
        let event:Option<ContactEvent>=sqlx::query_as("SELECT * FROM feishu_contact_inbox WHERE done=0 AND app_id=?1 AND attempted_at<?2 ORDER BY event_at LIMIT 1")
            .bind(&self.app_id).bind(now-30_000).fetch_optional(&self.pool).await.map_err(db)?;
        if let Some(event) = event {
            let result = self.process_event(&event).await;
            sqlx::query("UPDATE feishu_contact_inbox SET attempted_at=?2,done=?3,last_error=?4 WHERE event_id=?1")
                .bind(&event.event_id).bind(now).bind(result.is_ok()).bind(result.as_ref().err().map(ToString::to_string)).execute(&self.pool).await.map_err(db)?;
            // 失败事件不会饿死周期校对。
            if let Err(error) = result {
                tracing::warn!(error=%error,"飞书通讯录事件处理稍后重试");
            }
        }
        let subject:Option<String>=sqlx::query_scalar("SELECT e.subject FROM external_identities e LEFT JOIN feishu_user_states f ON f.subject=e.subject WHERE e.provider='feishu' AND COALESCE(f.attempted_at,0)<?1 ORDER BY COALESCE(f.attempted_at,0),e.subject LIMIT 1")
            .bind(now-SYNC_INTERVAL_MS).fetch_optional(&self.pool).await.map_err(db)?;
        if let Some(subject) = subject {
            self.sync_subject(&subject, 0).await?;
        }
        Ok(())
    }
    async fn process_event(&self, event: &ContactEvent) -> Result<()> {
        let subjects:Vec<String>=sqlx::query_scalar("SELECT e.subject FROM external_identities e LEFT JOIN feishu_user_states f ON f.subject=e.subject WHERE e.provider='feishu' AND ((?2='union_id' AND e.subject=?1) OR (?2='open_id' AND f.open_id=?1) OR (?2='user_id' AND f.directory_user_id=?1))")
            .bind(&event.user_id).bind(&event.id_type).fetch_all(&self.pool).await.map_err(db)?;
        for subject in subjects {
            let last:i64=sqlx::query_scalar("SELECT COALESCE((SELECT last_event_at FROM feishu_user_states WHERE subject=?1),0)")
                .bind(&subject).fetch_one(&self.pool).await.map_err(db)?;
            if event.event_at < last {
                continue;
            }
            if event.event_type == "contact.user.deleted_v3" {
                // 先读当前事实；迟到的离职事件不能覆盖已恢复的账号。
                match self.api.user(&subject, "union_id").await {
                    Ok(user) if user.union_id == subject => {
                        let mut tx = self.pool.begin().await.map_err(db)?;
                        persist_profile(&mut tx, &self.app_id, &user, event.event_at).await?;
                        tx.commit().await.map_err(db)?;
                    }
                    _ => {
                        sqlx::query("INSERT INTO feishu_user_states(subject,app_id,status,blocked,synced_at,attempted_at,last_event_at) VALUES(?1,?2,'deleted',1,?3,?3,?4) ON CONFLICT(subject) DO UPDATE SET status='deleted',blocked=1,synced_at=excluded.synced_at,attempted_at=excluded.attempted_at,last_event_at=excluded.last_event_at,last_error=NULL")
                            .bind(&subject).bind(&self.app_id).bind(Utc::now().timestamp_millis()).bind(event.event_at).execute(&self.pool).await.map_err(db)?;
                    }
                }
                self.enforce().await?;
            } else {
                self.sync_subject(&subject, event.event_at).await?;
            }
        }
        Ok(())
    }
    pub async fn receive(
        &self,
        headers: ApprovalEventHeaders<'_>,
        body: &[u8],
    ) -> Result<ApprovalWebhookReply> {
        let _maintenance = self.maintenance.read().await;
        let key = self
            .config
            .encrypt_key
            .as_deref()
            .ok_or_else(|| AppError::Config("请配置飞书 Encrypt Key".into()))?;
        let token = self
            .config
            .verification_token
            .as_deref()
            .ok_or_else(|| AppError::Config("请配置飞书 Verification Token".into()))?;
        let signed = match (headers.timestamp, headers.nonce, headers.signature) {
            (Some(ts), Some(_), Some(_)) => {
                verify_timestamp(ts)?;
                verify_signature(&headers, key, body)?;
                true
            }
            _ => false,
        };
        let envelope: Value = serde_json::from_slice(body)
            .map_err(|_| AppError::Validation("事件 JSON 非法".into()))?;
        let encrypted = envelope
            .get("encrypt")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("事件缺少 encrypt".into()))?;
        let event: Value = serde_json::from_slice(&decrypt_event(key, encrypted)?)
            .map_err(|_| AppError::Validation("事件解密内容非法".into()))?;
        verify_token(&event, token)?;
        if event.get("type").and_then(Value::as_str) == Some("url_verification") {
            return Ok(ApprovalWebhookReply::Challenge(
                string_at(&event, &["challenge"])
                    .ok_or_else(|| AppError::Validation("缺少 challenge".into()))?,
            ));
        }
        if !signed {
            return Err(AppError::Validation("事件缺少完整签名头".into()));
        }
        self.enqueue_event(&event).await?;
        Ok(ApprovalWebhookReply::Ack)
    }
    pub(crate) async fn enqueue_event(&self, event: &Value) -> Result<()> {
        if string_at(event, &["header.app_id"]).as_deref() != Some(self.app_id.as_str()) {
            return Err(AppError::Validation("飞书事件 App ID 不匹配".into()));
        }
        let kind = string_at(event, &["header.event_type"]).unwrap_or_default();
        if !matches!(
            kind.as_str(),
            "contact.user.updated_v3" | "contact.user.deleted_v3" | "contact.user.created_v3"
        ) {
            return Ok(());
        }
        let event_id = string_at(event, &["header.event_id"])
            .ok_or_else(|| AppError::Validation("事件缺少 ID".into()))?;
        let object = event
            .pointer("/event/object")
            .ok_or_else(|| AppError::Validation("事件缺少用户对象".into()))?;
        let (id_type, user_id) = ["union_id", "open_id", "user_id"]
            .into_iter()
            .find_map(|k| {
                string_at(object, &[k])
                    .or_else(|| object.get("user_id").and_then(|v| string_at(v, &[k])))
                    .filter(|v| !v.is_empty())
                    .map(|id| (k, id))
            })
            .ok_or_else(|| AppError::Validation("事件缺少用户 ID".into()))?;
        let event_at = string_at(event, &["header.create_time"])
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|t| *t > 0)
            .ok_or_else(|| AppError::Validation("事件缺少创建时间".into()))?;
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(event).map_err(|e| AppError::Internal(Box::new(e)))?)
        );
        sqlx::query("INSERT OR IGNORE INTO feishu_contact_inbox(event_id,app_id,user_id,id_type,event_type,event_at,payload_hash) VALUES(?1,?2,?3,?4,?5,?6,?7)")
            .bind(&event_id).bind(&self.app_id).bind(user_id).bind(id_type).bind(kind).bind(event_at).bind(&hash).execute(&self.pool).await.map_err(db)?;
        let prior: String =
            sqlx::query_scalar("SELECT payload_hash FROM feishu_contact_inbox WHERE event_id=?1")
                .bind(event_id)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        if prior != hash {
            return Err(AppError::Validation("拒绝载荷变化的通讯录事件重放".into()));
        }
        Ok(())
    }
}

async fn persist_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    app_id: &str,
    user: &DirectoryUser,
    event_at: i64,
) -> Result<()> {
    sqlx::query("INSERT INTO feishu_user_states(subject,app_id,open_id,directory_user_id,name,email,status,blocked,synced_at,attempted_at,last_event_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?10) ON CONFLICT(subject) DO UPDATE SET app_id=excluded.app_id,open_id=excluded.open_id,directory_user_id=excluded.directory_user_id,name=excluded.name,email=excluded.email,status=excluded.status,blocked=excluded.blocked,synced_at=excluded.synced_at,attempted_at=excluded.attempted_at,last_error=NULL,last_event_at=MAX(feishu_user_states.last_event_at,excluded.last_event_at)")
        .bind(&user.union_id).bind(app_id).bind(&user.open_id).bind(&user.user_id).bind(&user.name).bind(&user.email).bind(&user.status).bind(user.blocked()).bind(Utc::now().timestamp_millis()).bind(event_at)
        .execute(&mut **tx).await.map_err(db)?;
    Ok(())
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, FromRow)]
pub struct ContactEvent {
    pub event_id: String,
    pub app_id: String,
    pub user_id: String,
    pub id_type: String,
    pub event_type: String,
    pub event_at: i64,
    pub payload_hash: String,
    pub done: bool,
    pub attempted_at: i64,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, FromRow)]
pub struct DirectoryStateRow {
    pub subject: String,
    pub app_id: String,
    pub open_id: Option<String>,
    pub directory_user_id: Option<String>,
    pub name: String,
    pub email: String,
    pub status: String,
    pub blocked: bool,
    pub synced_at: Option<i64>,
    pub attempted_at: i64,
    pub last_error: Option<String>,
    pub last_event_at: i64,
}
pub async fn bindings(pool: &SqlitePool, user_id: &str) -> Result<Vec<FeishuBindingDto>> {
    type BindingRow = (
        String,
        String,
        String,
        String,
        bool,
        Option<i64>,
        Option<String>,
    );
    let rows: Vec<BindingRow> = sqlx::query_as("SELECT e.subject,COALESCE(f.name,''),COALESCE(f.email,''),COALESCE(f.status,'unknown'),COALESCE(f.blocked,0),f.synced_at,f.last_error FROM external_identities e LEFT JOIN feishu_user_states f ON e.subject=f.subject WHERE e.provider='feishu' AND e.user_id=?1 ORDER BY e.subject")
        .bind(user_id).fetch_all(pool).await.map_err(db)?;
    Ok(rows
        .into_iter()
        .map(|r| FeishuBindingDto {
            union_id: r.0,
            name: r.1,
            email: r.2,
            status: r.3,
            blocked: r.4,
            synced_at: r.5,
            last_error: r.6,
        })
        .collect())
}
fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::SqliteUserRepository;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;
    struct FakeApi {
        value: StdMutex<Option<DirectoryUser>>,
    }
    #[async_trait]
    impl DirectoryApi for FakeApi {
        async fn user(&self, _: &str, _: &str) -> Result<DirectoryUser> {
            self.value
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| AppError::Config("通讯录权限不足".into()))
        }
    }
    fn profile(status: &str) -> DirectoryUser {
        DirectoryUser {
            union_id: "union1".into(),
            open_id: Some("open1".into()),
            user_id: Some("employee1".into()),
            name: "测试用户".into(),
            email: "alice@example.com".into(),
            status: status.into(),
        }
    }
    async fn setup() -> (FeishuDirectoryService, Arc<FakeApi>) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let users = SqliteUserRepository::new(pool.clone());
        users
            .insert("u1", "alice", "alice@example.com", "hash", "user", false, 1)
            .await
            .unwrap();
        users
            .insert("u2", "bob", "bob@example.com", "hash", "user", false, 1)
            .await
            .unwrap();
        let api = Arc::new(FakeApi {
            value: StdMutex::new(Some(profile("active"))),
        });
        let config = FeishuApprovalConfig {
            encrypt_key: Some("secret-key".into()),
            verification_token: Some("verify".into()),
            ..Default::default()
        };
        (
            FeishuDirectoryService::new(pool, "app1".into(), config, api.clone(), None, None),
            api,
        )
    }
    fn lookup() -> FeishuLookupRequest {
        FeishuLookupRequest {
            user_id: "open1".into(),
            id_type: "open_id".into(),
        }
    }
    fn event(id: &str, kind: &str, time: i64) -> Value {
        json!({"header":{"app_id":"app1","event_id":id,"event_type":kind,"create_time":time.to_string()},"event":{"object":{"user_id":{"union_id":"union1","open_id":"open1","user_id":"employee1"}}}})
    }

    #[test]
    fn status_requires_explicit_fields_and_checks_all_blocking_flags() {
        let base = json!({"union_id":"union1","name":"Alice","status":{"is_activated":true,"is_frozen":false,"is_resigned":false,"is_exited":false,"is_unjoin":false}});
        assert!(!parse_directory_user(&base).unwrap().blocked());
        for (flag, expected) in [
            ("is_frozen", "frozen"),
            ("is_resigned", "resigned"),
            ("is_exited", "deleted"),
            ("is_unjoin", "inactive"),
        ] {
            let mut value = base.clone();
            value["status"][flag] = json!(true);
            assert_eq!(parse_directory_user(&value).unwrap().status, expected);
        }
        let mut inactive = base.clone();
        inactive["status"]["is_activated"] = json!(false);
        assert_eq!(parse_directory_user(&inactive).unwrap().status, "inactive");
        assert!(parse_directory_user(&json!({"union_id":"union1","status":{}})).is_err());
        assert!(parse_directory_user(&json!({"union_id":"union1"})).is_err());
    }
    #[tokio::test]
    async fn binding_conflicts_are_rejected_and_never_reassign_an_identity() {
        let (service, api) = setup().await;
        service.bind("u1", &lookup()).await.unwrap();
        service.bind("u1", &lookup()).await.unwrap();
        assert!(matches!(
            service.bind("u2", &lookup()).await,
            Err(AppError::DuplicateResource(_))
        ));
        let mut other = profile("active");
        other.union_id = "union2".into();
        *api.value.lock().unwrap() = Some(other);
        assert!(matches!(
            service.bind("u1", &lookup()).await,
            Err(AppError::DuplicateResource(_))
        ));
        let owner: String =
            sqlx::query_scalar("SELECT user_id FROM external_identities WHERE subject='union1'")
                .fetch_one(&service.pool)
                .await
                .unwrap();
        assert_eq!(owner, "u1");
        assert_eq!(bindings(&service.pool, "u1").await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn blocked_binding_revokes_sessions_and_recovery_preserves_manual_disable_and_grants() {
        let (service, api) = setup().await;
        let sessions = SqliteSessionRepository::new(service.pool.clone());
        sessions
            .create(
                "s1",
                "u1",
                "token",
                None,
                None,
                Utc::now().timestamp_millis() + 60_000,
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES('g1','ops','10.0.0.0/8',0,0)").execute(&service.pool).await.unwrap();
        sqlx::query("INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,created_at,updated_at) VALUES('a1','i1','u1','g1',1,0,0)").execute(&service.pool).await.unwrap();
        *api.value.lock().unwrap() = Some(profile("frozen"));
        service.bind("u1", &lookup()).await.unwrap();
        let users = SqliteUserRepository::new(service.pool.clone());
        assert!(matches!(
            users.ensure_available("u1").await,
            Err(AppError::AccountDisabled)
        ));
        assert!(sessions
            .find_active_by_token_hash("token", Utc::now().timestamp_millis())
            .await
            .unwrap()
            .is_none());
        users.update_status("u1", "disabled").await.unwrap();
        *api.value.lock().unwrap() = Some(profile("active"));
        service.sync_user("u1").await.unwrap();
        assert!(!bindings(&service.pool, "u1").await.unwrap()[0].blocked);
        assert!(matches!(
            users.ensure_available("u1").await,
            Err(AppError::AccountDisabled)
        ));
        let expiry: i64 = sqlx::query_scalar("SELECT expires_at FROM access_grants WHERE id='a1'")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        assert_eq!(expiry, 1);
        users.update_status("u1", "active").await.unwrap();
        users.ensure_available("u1").await.unwrap();
        assert!(sessions
            .find_active_by_token_hash("token", Utc::now().timestamp_millis())
            .await
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn failed_sync_preserves_state_and_records_error() {
        let (service, api) = setup().await;
        *api.value.lock().unwrap() = Some(profile("frozen"));
        service.bind("u1", &lookup()).await.unwrap();
        let before = bindings(&service.pool, "u1").await.unwrap().remove(0);
        *api.value.lock().unwrap() = None;
        assert!(service.sync_user("u1").await.is_err());
        let after = bindings(&service.pool, "u1").await.unwrap().remove(0);
        assert!(after.blocked);
        assert_eq!(after.synced_at, before.synced_at);
        assert!(after.last_error.is_some());
    }
    #[tokio::test]
    async fn unbound_manual_accounts_are_not_changed_or_auto_bound_by_sync() {
        let (service, api) = setup().await;
        // 通讯录有同邮箱的冻结用户，也不能据此绑定或限制手工账号。
        let mut remote = profile("frozen");
        remote.union_id = "unbound-union".into();
        let mut tx = service.pool.begin().await.unwrap();
        persist_profile(&mut tx, "app1", &remote, 0).await.unwrap();
        tx.commit().await.unwrap();
        let sessions = SqliteSessionRepository::new(service.pool.clone());
        let now = Utc::now().timestamp_millis();
        sessions
            .create(
                "manual-session",
                "u1",
                "manual-token",
                None,
                None,
                now + 60_000,
            )
            .await
            .unwrap();
        *api.value.lock().unwrap() = None;
        let mut e = event("unbound-event", "contact.user.deleted_v3", 100);
        e["event"]["object"]["user_id"]["union_id"] = json!("unbound-union");
        service.enqueue_event(&e).await.unwrap();
        assert_eq!(service.queue_all().await.unwrap(), 0);
        service.work_once().await.unwrap();
        let users = SqliteUserRepository::new(service.pool.clone());
        users.ensure_available("u1").await.unwrap();
        assert!(bindings(&service.pool, "u1").await.unwrap().is_empty());
        assert!(sessions
            .find_active_by_token_hash("manual-token", now)
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            sessions
                .renew_active_by_token_hash("manual-token", "u1", now, now + 120_000)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            users.find_by_id("u1").await.unwrap().unwrap().status,
            "active"
        );
    }

    #[tokio::test]
    async fn periodic_sync_discovers_preexisting_login_bindings() {
        let (service, _) = setup().await;
        sqlx::query("INSERT INTO external_identities VALUES('feishu','union1','u1',0)")
            .execute(&service.pool)
            .await
            .unwrap();
        assert_eq!(
            bindings(&service.pool, "u1").await.unwrap()[0].status,
            "unknown"
        );
        service.work_once().await.unwrap();
        assert_eq!(
            bindings(&service.pool, "u1").await.unwrap()[0].status,
            "active"
        );
    }
    #[tokio::test]
    async fn events_validate_app_id_dedupe_payloads_and_apply_deletion() {
        let (service, api) = setup().await;
        service.bind("u1", &lookup()).await.unwrap();
        let mut e = event("event1", "contact.user.deleted_v3", 100);
        e["header"]["app_id"] = json!("other");
        assert!(service.enqueue_event(&e).await.is_err());
        e["header"]["app_id"] = json!("app1");
        service.enqueue_event(&e).await.unwrap();
        service.enqueue_event(&e).await.unwrap();
        let mut changed = e.clone();
        changed["event"]["object"]["name"] = json!("changed");
        assert!(service.enqueue_event(&changed).await.is_err());
        *api.value.lock().unwrap() = None;
        service.work_once().await.unwrap();
        let b = bindings(&service.pool, "u1").await.unwrap().remove(0);
        assert!(b.blocked);
        assert_eq!(b.status, "deleted");
        let done: bool =
            sqlx::query_scalar("SELECT done FROM feishu_contact_inbox WHERE event_id='event1'")
                .fetch_one(&service.pool)
                .await
                .unwrap();
        assert!(done);
        *api.value.lock().unwrap() = Some(profile("active"));
        service
            .enqueue_event(&event("event2", "contact.user.updated_v3", 200))
            .await
            .unwrap();
        service.work_once().await.unwrap();
        assert!(!bindings(&service.pool, "u1").await.unwrap()[0].blocked);
        // 旧离职事件即使在恢复后重投，也不会重新禁用。
        *api.value.lock().unwrap() = None;
        service
            .enqueue_event(&event("event0", "contact.user.deleted_v3", 50))
            .await
            .unwrap();
        service.work_once().await.unwrap();
        assert!(!bindings(&service.pool, "u1").await.unwrap()[0].blocked);
    }
    #[tokio::test]
    async fn callback_requires_valid_signature_and_accepts_encrypted_challenge() {
        use cbc::cipher::{BlockEncryptMut, KeyIvInit};
        let (service, _) = setup().await;
        let encode = |value: Value| {
            use base64::Engine;
            let key: [u8; 32] = Sha256::digest(b"secret-key").into();
            let iv = [7u8; 16];
            let plain = serde_json::to_vec(&value).unwrap();
            let mut buf = vec![0; plain.len() + 16];
            buf[..plain.len()].copy_from_slice(&plain);
            let enc = cbc::Encryptor::<aes::Aes256>::new(&key.into(), &iv.into())
                .encrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buf, plain.len())
                .unwrap();
            let mut bytes = iv.to_vec();
            bytes.extend_from_slice(enc);
            serde_json::to_vec(
                &json!({"encrypt":base64::engine::general_purpose::STANDARD.encode(bytes)}),
            )
            .unwrap()
        };
        let body = encode(json!({"type":"url_verification","token":"verify","challenge":"test"}));
        assert!(matches!(
            service
                .receive(
                    ApprovalEventHeaders {
                        timestamp: None,
                        nonce: None,
                        signature: None
                    },
                    &body
                )
                .await
                .unwrap(),
            ApprovalWebhookReply::Challenge(_)
        ));
        let mut e = event("signed1", "contact.user.updated_v3", 100);
        e["header"]["token"] = json!("verify");
        let body = encode(e);
        let ts = Utc::now().timestamp().to_string();
        let nonce = "nonce";
        assert!(service
            .receive(
                ApprovalEventHeaders {
                    timestamp: Some(&ts),
                    nonce: Some(nonce),
                    signature: Some("bad")
                },
                &body
            )
            .await
            .is_err());
        let mut digest = Sha256::new();
        digest.update(ts.as_bytes());
        digest.update(nonce);
        digest.update("secret-key");
        digest.update(&body);
        let sig = format!("{:x}", digest.finalize());
        service
            .receive(
                ApprovalEventHeaders {
                    timestamp: Some(&ts),
                    nonce: Some(nonce),
                    signature: Some(&sig),
                },
                &body,
            )
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM feishu_contact_inbox")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
