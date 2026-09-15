//! 飞书审批事件安全接收、durable inbox worker 与实例二次确认。

use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use aes::Aes256;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpn_core::{service::PasswordHasher, AppError, Result};

use crate::{
    config::{FeishuApprovalConfig, FeishuConfig},
    repositories::{
        ApprovalIdentity, ApprovedGrant, EnqueueResult, InboxRow, SqliteAccessGrantRepository,
    },
};

use super::NetworkAclService;

const EVENT_WINDOW_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct ApprovalEventHeaders<'a> {
    pub timestamp: Option<&'a str>,
    pub nonce: Option<&'a str>,
    pub signature: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalWebhookReply {
    Ack,
    Challenge(String),
}

#[derive(Debug, Clone)]
pub struct ApprovalInstance {
    pub approval_code: String,
    pub status: String,
    pub applicant_id: String,
    pub applicant_id_type: String,
    pub form: Value,
}

#[derive(Debug, Clone)]
pub struct ApprovalApplicant {
    pub union_id: String,
    pub email: String,
}

#[async_trait]
pub trait FeishuApprovalApi: Send + Sync {
    async fn get_instance(&self, instance_code: &str) -> Result<ApprovalInstance>;
    async fn get_applicant(&self, user_id: &str, user_id_type: &str) -> Result<ApprovalApplicant>;
}

#[derive(Clone)]
pub struct ReqwestFeishuApprovalApi {
    http: reqwest::Client,
    feishu: FeishuConfig,
}

impl ReqwestFeishuApprovalApi {
    pub fn new(feishu: FeishuConfig) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(internal)?,
            feishu,
        })
    }

    pub async fn subscribe(&self, approval_code: &str) -> Result<()> {
        let token = self
            .tenant_token()
            .await
            .map_err(|_| AppError::Config("飞书应用鉴权失败，请检查配置和网络".into()))?;
        let mut url =
            reqwest::Url::parse("https://open.feishu.cn/open-apis/approval/v4/approvals/")
                .map_err(|_| AppError::Config("飞书订阅地址配置错误".into()))?;
        url.path_segments_mut()
            .map_err(|_| AppError::Config("飞书订阅地址配置错误".into()))?
            .pop_if_empty()
            .push(approval_code)
            .push("subscribe");
        #[derive(Deserialize)]
        struct Response {
            code: i64,
        }
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| AppError::Config("飞书订阅请求失败，请检查网络后重试".into()))?;
        let status = response.status();
        let response = response
            .json::<Response>()
            .await
            .map_err(|_| AppError::Config("飞书订阅响应格式异常".into()))?;
        ensure_subscription_success(status, response.code)
    }

    pub(crate) async fn tenant_token(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Request<'a> {
            app_id: &'a str,
            app_secret: &'a str,
        }
        #[derive(Deserialize)]
        struct Response {
            code: i64,
            tenant_access_token: Option<String>,
        }
        let response = self
            .http
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
            .json(&Request {
                app_id: self.feishu.app_id.as_deref().ok_or_else(disabled)?,
                app_secret: self.feishu.app_secret.as_deref().ok_or_else(disabled)?,
            })
            .send()
            .await
            .map_err(internal)?
            .error_for_status()
            .map_err(internal)?
            .json::<Response>()
            .await
            .map_err(internal)?;
        if response.code != 0 {
            tracing::warn!(code = response.code, "飞书 tenant token 请求失败");
            return Err(AppError::Config("飞书 tenant token 请求失败".into()));
        }
        response
            .tenant_access_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| AppError::Validation("飞书未返回 tenant token".into()))
    }
}

#[async_trait]
impl FeishuApprovalApi for ReqwestFeishuApprovalApi {
    async fn get_instance(&self, instance_code: &str) -> Result<ApprovalInstance> {
        let token = self.tenant_token().await?;
        let url = format!("https://open.feishu.cn/open-apis/approval/v4/instances/{instance_code}");
        let value = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(internal)?
            .error_for_status()
            .map_err(internal)?
            .json::<Value>()
            .await
            .map_err(internal)?;
        ensure_feishu_success(&value, "审批实例")?;
        parse_instance(&value)
    }

    async fn get_applicant(&self, user_id: &str, user_id_type: &str) -> Result<ApprovalApplicant> {
        if !matches!(user_id_type, "open_id" | "user_id" | "union_id") {
            return Err(AppError::Validation("飞书申请人 ID 类型非法".into()));
        }
        let token = self.tenant_token().await?;
        let url = format!("https://open.feishu.cn/open-apis/contact/v3/users/{user_id}");
        let value = self
            .http
            .get(url)
            .query(&[("user_id_type", user_id_type)])
            .bearer_auth(token)
            .send()
            .await
            .map_err(internal)?
            .error_for_status()
            .map_err(internal)?
            .json::<Value>()
            .await
            .map_err(internal)?;
        ensure_feishu_success(&value, "申请人详情")?;
        let user = value
            .pointer("/data/user")
            .ok_or_else(|| AppError::Validation("飞书未返回申请人详情".into()))?;
        let union_id = string_at(user, &["union_id"])
            .ok_or_else(|| AppError::Validation("飞书申请人缺少 union_id".into()))?;
        let email = string_at(user, &["enterprise_email", "email"])
            .map(|email| email.to_ascii_lowercase())
            .filter(|email| email.contains('@'))
            .ok_or_else(|| AppError::Validation("飞书申请人缺少企业邮箱".into()))?;
        Ok(ApprovalApplicant { union_id, email })
    }
}

#[derive(Clone)]
pub struct FeishuApprovalService {
    config: FeishuApprovalConfig,
    repo: SqliteAccessGrantRepository,
    api: Arc<dyn FeishuApprovalApi>,
    hasher: Arc<dyn PasswordHasher>,
    network_acl: Option<Arc<NetworkAclService>>,
    directory: Option<Arc<super::FeishuDirectoryService>>,
    maintenance: Arc<RwLock<()>>,
}

impl FeishuApprovalService {
    pub fn new(
        config: FeishuApprovalConfig,
        repo: SqliteAccessGrantRepository,
        api: Arc<dyn FeishuApprovalApi>,
        hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            config,
            repo,
            api,
            hasher,
            network_acl: None,
            directory: None,
            maintenance: Arc::new(RwLock::new(())),
        }
    }

    pub fn with_directory(mut self, service: Arc<super::FeishuDirectoryService>) -> Self {
        self.directory = Some(service);
        self
    }

    pub fn with_network_acl(mut self, network_acl: Arc<NetworkAclService>) -> Self {
        self.network_acl = Some(network_acl);
        self
    }

    /// 暂停审批 worker，供整库恢复在一个互斥窗口内替换事实数据。
    pub async fn pause_worker(&self) -> OwnedRwLockWriteGuard<()> {
        self.maintenance.clone().write_owned().await
    }

    /// 验签与解密必须发生在解析/落库之前；成功落 durable inbox 后即可快速 ACK。
    pub async fn receive(
        &self,
        headers: ApprovalEventHeaders<'_>,
        raw_body: &[u8],
    ) -> Result<ApprovalWebhookReply> {
        if !self.config.enabled() {
            return Err(disabled());
        }
        // 与整库恢复互斥：恢复先持写锁，期间到达的 webhook 会在恢复完成后才入箱并 ACK，
        // 避免“已确认事件刚落库就被恢复事务删除”。
        let _maintenance = self.maintenance.read().await;
        let encrypt_key = self.config.encrypt_key.as_deref().ok_or_else(disabled)?;
        let signed = match (headers.timestamp, headers.nonce, headers.signature) {
            (Some(timestamp), Some(_), Some(_)) => {
                verify_timestamp(timestamp)?;
                verify_signature(&headers, encrypt_key, raw_body)?;
                true
            }
            _ => false,
        };
        let envelope: Value = serde_json::from_slice(raw_body)
            .map_err(|_| AppError::Validation("飞书审批事件 JSON 非法".into()))?;
        let encrypted = envelope
            .get("encrypt")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("飞书审批事件缺少 encrypt".into()))?;
        let plain = decrypt_event(encrypt_key, encrypted)?;
        let event: Value = serde_json::from_slice(&plain)
            .map_err(|_| AppError::Validation("飞书审批事件密文内容非法".into()))?;
        verify_token(
            &event,
            self.config
                .verification_token
                .as_deref()
                .ok_or_else(disabled)?,
        )?;
        if string_at(&event, &["type"]).as_deref() == Some("url_verification") {
            if let Some(challenge) = event.get("challenge").and_then(Value::as_str) {
                return Ok(ApprovalWebhookReply::Challenge(challenge.to_string()));
            }
        }
        if !signed {
            return Err(AppError::Validation(
                "飞书审批事件缺少完整 X-Lark 签名头".into(),
            ));
        }

        if string_at(&event, &["header.event_type"])
            .is_some_and(|kind| kind.starts_with("contact."))
        {
            let directory = self
                .directory
                .as_ref()
                .ok_or_else(|| AppError::Config("飞书用户状态同步未配置".into()))?;
            directory.enqueue_event(&event).await?;
            return Ok(ApprovalWebhookReply::Ack);
        }

        let meta = parse_event_meta(&event)?;
        if meta.approval_code != self.config.approval_code.as_deref().ok_or_else(disabled)?
            || meta.status != "APPROVED"
        {
            return Ok(ApprovalWebhookReply::Ack);
        }
        // 飞书可能用新的 event_id/create_time 重投同一实例；移除纯投递字段后再散列，
        // 既保证实例重投幂等，也能让其余已验签载荷变化触发冲突告警。
        let payload_hash = idempotency_payload_hash(event.clone())?;
        // Worker 只需元数据；不持久化完整解密事件或潜在表单敏感值。
        let payload = serde_json::to_string(&serde_json::json!({
            "event_id": meta.event_id,
            "instance_code": meta.instance_code,
        }))
        .map_err(|error| AppError::Internal(Box::new(error)))?;
        match self
            .repo
            .enqueue(&meta.event_id, &meta.instance_code, &payload_hash, &payload)
            .await?
        {
            EnqueueResult::Inserted | EnqueueResult::Duplicate => Ok(ApprovalWebhookReply::Ack),
            EnqueueResult::Conflict => {
                tracing::warn!(
                    event_id = %meta.event_id,
                    instance_code = %meta.instance_code,
                    "拒绝载荷发生变化的飞书审批重放"
                );
                Err(AppError::Validation(
                    "飞书审批事件 ID 对应载荷发生变化".into(),
                ))
            }
        }
    }

    pub fn spawn_worker(&self) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                if let Err(error) = service.work_once().await {
                    tracing::error!(error = ?error, "飞书审批 worker 扫描失败");
                }
            }
        });
    }

    pub async fn work_once(&self) -> Result<bool> {
        // 从认领到最终状态更新都处于同一读锁窗口；备份恢复持写锁，二者不会交叉提交。
        let _maintenance = self.maintenance.read().await;
        let Some(item) = self.repo.claim_due().await? else {
            return Ok(false);
        };
        match self.process(&item).await {
            Ok(()) => self.repo.finish(&item.event_id).await?,
            Err(error) if permanent(&error) => {
                tracing::warn!(event_id = %item.event_id, "飞书审批任务被拒绝");
                self.repo
                    .reject(&item.event_id, error_class(&error))
                    .await?;
            }
            Err(error) => {
                tracing::warn!(event_id = %item.event_id, "飞书审批任务稍后重试");
                self.repo
                    .retry(&item.event_id, item.attempts, error_class(&error))
                    .await?;
            }
        }
        Ok(true)
    }

    async fn process(&self, item: &InboxRow) -> Result<()> {
        let instance = self.api.get_instance(&item.instance_code).await?;
        if instance.status != "APPROVED" {
            return Err(AppError::Validation("审批实例未处于 APPROVED".into()));
        }
        if instance.approval_code != self.config.approval_code.as_deref().ok_or_else(disabled)? {
            return Err(AppError::Validation("审批定义不匹配".into()));
        }
        let fields = parse_form(
            &instance.form,
            self.config
                .group_control_id
                .as_deref()
                .ok_or_else(disabled)?,
            self.config
                .expiry_control_id
                .as_deref()
                .ok_or_else(disabled)?,
            self.config
                .reason_control_id
                .as_deref()
                .ok_or_else(disabled)?,
        )?;
        if fields.expires_at <= Utc::now().timestamp_millis() {
            return Err(AppError::Validation(
                "审批授权到期日必须晚于当前时间".into(),
            ));
        }
        let applicant = self
            .api
            .get_applicant(&instance.applicant_id, &instance.applicant_id_type)
            .await?;
        let preferred = applicant
            .email
            .split('@')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("feishu");
        let suffix = &hex_sha256(applicant.union_id.as_bytes())[..8];
        let password = Uuid::now_v7().to_string();
        let password_hash = self.hasher.hash(&password)?;
        let new_user_id = Uuid::now_v7().to_string();
        self.repo
            .apply_approved(ApprovedGrant {
                instance_code: &item.instance_code,
                group_ids: fields.group_ids,
                expires_at: fields.expires_at,
                reason: &fields.reason,
                identity: ApprovalIdentity {
                    subject: &applicant.union_id,
                    email: &applicant.email,
                    preferred_username: preferred,
                    username_suffix: suffix,
                    new_user_id: &new_user_id,
                    password_hash: &password_hash,
                },
            })
            .await?;
        if let Some(network_acl) = &self.network_acl {
            network_acl.refresh().await?;
        }
        Ok(())
    }
}

struct FormFields {
    group_ids: Vec<String>,
    expires_at: i64,
    reason: String,
}

fn parse_form(
    value: &Value,
    group_id: &str,
    expiry_id: &str,
    reason_id: &str,
) -> Result<FormFields> {
    let form = if let Some(serialized) = value.as_str() {
        serde_json::from_str::<Value>(serialized)
            .map_err(|_| AppError::Validation("审批表单 JSON 字符串非法".into()))?
    } else {
        value.clone()
    };
    let widgets = form
        .as_array()
        .or_else(|| form.get("form").and_then(Value::as_array))
        .or_else(|| form.get("widget_list").and_then(Value::as_array))
        .ok_or_else(|| AppError::Validation("审批表单不是控件数组".into()))?;
    let group = unique_widget(widgets, group_id)?;
    let expiry = unique_widget(widgets, expiry_id)?;
    let reason = unique_widget(widgets, reason_id)?;
    Ok(FormFields {
        group_ids: group_option_ids(group)?,
        expires_at: exclusive_expiry(widget_value(expiry)?)?,
        reason: scalar_text(widget_value(reason)?)?,
    })
}

fn unique_widget<'a>(widgets: &'a [Value], control_id: &str) -> Result<&'a Value> {
    let matches: Vec<&Value> = widgets
        .iter()
        .filter(|widget| {
            string_at(widget, &["id", "widget_id", "widgetId", "control_id"]).as_deref()
                == Some(control_id)
        })
        .collect();
    match matches.as_slice() {
        [widget] => Ok(*widget),
        [] => Err(AppError::Validation(format!(
            "审批表单缺少控件 ID {control_id}"
        ))),
        _ => Err(AppError::Validation(format!(
            "审批表单控件 ID {control_id} 重复"
        ))),
    }
}

fn widget_value(widget: &Value) -> Result<&Value> {
    widget
        .get("value")
        .or_else(|| widget.get("values"))
        .or_else(|| widget.get("option"))
        .ok_or_else(|| AppError::Validation("审批控件缺少 value".into()))
}

// 实例详情中的 value 是显示文案；option 才携带已选项的稳定 key。
fn group_option_ids(widget: &Value) -> Result<Vec<String>> {
    if let Some(options) = widget.get("option") {
        let selected = match options {
            Value::Array(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let keys: Result<Vec<Value>> = selected
            .iter()
            .map(|option| {
                string_at(option, &["key", "id", "option_id"])
                    .filter(|key| !key.trim().is_empty())
                    .map(Value::String)
                    .ok_or_else(|| AppError::Validation("用户组选项缺少稳定 ID".into()))
            })
            .collect();
        return option_ids(&Value::Array(keys?));
    }
    option_ids(widget_value(widget)?)
}

fn option_ids(value: &Value) -> Result<Vec<String>> {
    if let Value::String(text) = value {
        if let Ok(nested @ (Value::Array(_) | Value::Object(_))) = serde_json::from_str(text) {
            return option_ids(&nested);
        }
    }
    let selected = match value {
        Value::Array(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if selected.is_empty() {
        return Err(AppError::Validation("至少选择一个用户组".into()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for selected in selected {
        let id = match selected {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Object(_) => string_at(selected, &["id", "key", "value", "option_id"]),
            _ => None,
        }
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| AppError::Validation("用户组控件没有稳定选项 ID".into()))?;
        ids.insert(id.trim().to_string());
    }
    Ok(ids.into_iter().collect())
}

fn scalar_text(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value.trim().to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Object(_) => string_at(value, &["text", "value"])
            .ok_or_else(|| AppError::Validation("事由控件值非法".into())),
        _ => Err(AppError::Validation("事由控件值非法".into())),
    }
}

fn exclusive_expiry(value: &Value) -> Result<i64> {
    let date = parse_local_date(value)?;
    let next = date
        .succ_opt()
        .ok_or_else(|| AppError::Validation("到期日超出范围".into()))?;
    let shanghai = FixedOffset::east_opt(8 * 3600).expect("fixed timezone");
    let midnight = shanghai
        .from_local_datetime(&next.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .ok_or_else(|| AppError::Validation("到期日无法转换".into()))?;
    Ok(midnight.timestamp_millis())
}

fn parse_local_date(value: &Value) -> Result<NaiveDate> {
    let shanghai = FixedOffset::east_opt(8 * 3600).expect("fixed timezone");
    let from_millis = |millis| {
        DateTime::<Utc>::from_timestamp_millis(millis)
            .map(|date| date.with_timezone(&shanghai).date_naive())
    };
    let parsed = match value {
        Value::String(value) => {
            let trimmed = value.trim();
            if let Ok(millis) = trimmed.parse::<i64>() {
                from_millis(millis)
            } else if let Ok(nested) = serde_json::from_str::<Value>(trimmed) {
                return parse_local_date(&nested);
            } else if trimmed.len() == 10 {
                NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()
            } else {
                DateTime::parse_from_rfc3339(trimmed)
                    .ok()
                    .map(|date| date.with_timezone(&shanghai).date_naive())
            }
        }
        Value::Number(number) => number.as_i64().and_then(from_millis),
        Value::Object(_) => {
            let nested = value
                .get("value")
                .or_else(|| value.get("date"))
                .or_else(|| value.get("timestamp"))
                .ok_or_else(|| AppError::Validation("到期日控件值非法".into()))?;
            return parse_local_date(nested);
        }
        Value::Array(values) if values.len() == 1 => return parse_local_date(&values[0]),
        _ => None,
    };
    parsed.ok_or_else(|| AppError::Validation("到期日控件值非法".into()))
}

fn parse_instance(value: &Value) -> Result<ApprovalInstance> {
    let data = value.get("data").unwrap_or(value);
    let instance = data.get("instance").unwrap_or(data);
    let applicant_id_type = if string_at(instance, &["open_id"]).is_some() {
        "open_id"
    } else {
        "user_id"
    };
    let applicant_id = string_at(instance, &["open_id", "user_id", "applicant_id"])
        .ok_or_else(|| AppError::Validation("审批实例缺少申请人 ID".into()))?;
    let form = instance
        .get("form")
        .or_else(|| data.get("form"))
        .cloned()
        .ok_or_else(|| AppError::Validation("审批实例缺少 form".into()))?;
    Ok(ApprovalInstance {
        approval_code: string_at(instance, &["approval_code"])
            .or_else(|| string_at(data, &["approval_code"]))
            .ok_or_else(|| AppError::Validation("审批实例缺少 approval_code".into()))?,
        status: string_at(instance, &["status"])
            .ok_or_else(|| AppError::Validation("审批实例缺少 status".into()))?,
        applicant_id,
        applicant_id_type: applicant_id_type.to_string(),
        form,
    })
}

pub(crate) fn verify_timestamp(timestamp: &str) -> Result<()> {
    let timestamp = timestamp
        .parse::<i64>()
        .map_err(|_| AppError::Validation("飞书事件时间戳非法".into()))?;
    if Utc::now().timestamp().abs_diff(timestamp) > EVENT_WINDOW_SECS as u64 {
        return Err(AppError::Validation("飞书事件超出允许时间窗".into()));
    }
    Ok(())
}

pub(crate) fn verify_signature(
    headers: &ApprovalEventHeaders<'_>,
    encrypt_key: &str,
    raw_body: &[u8],
) -> Result<()> {
    let timestamp = headers
        .timestamp
        .ok_or_else(|| AppError::Validation("飞书事件缺少时间戳".into()))?;
    let nonce = headers
        .nonce
        .ok_or_else(|| AppError::Validation("飞书事件缺少 nonce".into()))?;
    let signature = headers
        .signature
        .ok_or_else(|| AppError::Validation("飞书事件缺少签名".into()))?;
    let mut digest = Sha256::new();
    digest.update(timestamp.as_bytes());
    digest.update(nonce.as_bytes());
    digest.update(encrypt_key.as_bytes());
    digest.update(raw_body);
    let expected = hex_bytes(&digest.finalize());
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return Err(AppError::Validation("飞书事件签名无效".into()));
    }
    Ok(())
}

pub(crate) fn decrypt_event(encrypt_key: &str, encrypted: &str) -> Result<Vec<u8>> {
    type Decryptor = cbc::Decryptor<Aes256>;
    let mut ciphertext = STANDARD
        .decode(encrypted)
        .map_err(|_| AppError::Validation("飞书事件密文 base64 非法".into()))?;
    if ciphertext.len() <= 16 {
        return Err(AppError::Validation("飞书事件密文过短".into()));
    }
    let iv: [u8; 16] = ciphertext[..16]
        .try_into()
        .map_err(|_| AppError::Validation("飞书事件 IV 非法".into()))?;
    let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
    let plain = Decryptor::new(&key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext[16..])
        .map_err(|_| AppError::Validation("飞书事件密文解密失败".into()))?;
    Ok(plain.to_vec())
}

pub(crate) fn verify_token(value: &Value, expected: &str) -> Result<()> {
    let supplied = string_at(value, &["token", "header.token"])
        .ok_or_else(|| AppError::Validation("飞书事件缺少 verification token".into()))?;
    if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        return Err(AppError::Validation(
            "飞书事件 verification token 无效".into(),
        ));
    }
    Ok(())
}

pub(crate) fn string_at(value: &Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let mut current = value;
        let mut found = true;
        for segment in path.split('.') {
            if let Some(next) = current.get(segment) {
                current = next;
            } else {
                found = false;
                break;
            }
        }
        if found {
            if let Some(text) = current.as_str().filter(|text| !text.trim().is_empty()) {
                return Some(text.trim().to_string());
            }
        }
    }
    None
}

fn ensure_feishu_success(value: &Value, resource: &str) -> Result<()> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        // 远端业务错误可能是限流、权限暂态或服务故障，保留 inbox 重试而非永久拒绝。
        return Err(AppError::Config(format!("飞书{resource}请求失败")));
    }
    Ok(())
}

fn hex_sha256(value: &[u8]) -> String {
    hex_bytes(&Sha256::digest(value))
}

fn idempotency_payload_hash(mut value: Value) -> Result<String> {
    if let Some(root) = value.as_object_mut() {
        for key in ["event_id", "uuid", "ts"] {
            root.remove(key);
        }
        if let Some(header) = root.get_mut("header").and_then(Value::as_object_mut) {
            header.remove("event_id");
            header.remove("create_time");
        }
        if let Some(event) = root.get_mut("event").and_then(Value::as_object_mut) {
            event.remove("uuid");
        }
    }
    let canonical =
        serde_json::to_vec(&value).map_err(|error| AppError::Internal(Box::new(error)))?;
    Ok(hex_sha256(&canonical))
}

fn hex_bytes(value: &[u8]) -> String {
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn permanent(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_)
            | AppError::DuplicateResource(_)
            | AppError::AccountDisabled
            | AppError::UserNotFound
    )
}

fn error_class(error: &AppError) -> &'static str {
    match error {
        AppError::Validation(_) => "validation_rejected",
        AppError::DuplicateResource(_) => "identity_conflict",
        AppError::AccountDisabled => "account_disabled",
        AppError::UserNotFound => "identity_missing",
        AppError::Database(_) => "database_error",
        _ => "upstream_or_internal_error",
    }
}

fn ensure_subscription_success(status: reqwest::StatusCode, code: i64) -> Result<()> {
    if code != 0 {
        return Err(AppError::Config(format!("飞书订阅失败（错误码 {code}）")));
    }
    if !status.is_success() {
        return Err(AppError::Config("飞书订阅请求返回异常状态".into()));
    }
    Ok(())
}

fn internal(error: reqwest::Error) -> AppError {
    AppError::Internal(Box::new(error))
}

fn disabled() -> AppError {
    AppError::Config("飞书审批未配置".into())
}

struct EventMeta {
    event_id: String,
    approval_code: String,
    instance_code: String,
    status: String,
}

fn parse_event_meta(value: &Value) -> Result<EventMeta> {
    let event = value.get("event").unwrap_or(value);
    Ok(EventMeta {
        event_id: string_at(
            value,
            &["header.event_id", "event_id", "uuid", "event.uuid"],
        )
        .ok_or_else(|| AppError::Validation("飞书审批事件缺少 event_id/uuid".into()))?,
        approval_code: string_at(event, &["approval_code"])
            .ok_or_else(|| AppError::Validation("飞书审批事件缺少 approval_code".into()))?,
        instance_code: string_at(event, &["instance_code", "approval_instance_code"])
            .ok_or_else(|| AppError::Validation("飞书审批事件缺少 instance_code".into()))?,
        status: string_at(event, &["status", "instance_status"])
            .ok_or_else(|| AppError::Validation("飞书审批事件缺少 status".into()))?,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn subscription_only_accepts_confirmed_success() {
        use reqwest::StatusCode;
        assert!(super::ensure_subscription_success(StatusCode::OK, 0).is_ok());
        assert!(super::ensure_subscription_success(StatusCode::BAD_REQUEST, 0).is_err());
        for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
            let error = super::ensure_subscription_success(status, 1390007).unwrap_err();
            assert!(error.to_string().contains("1390007"));
        }
    }
    use super::*;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};
    use serde_json::json;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    struct UnusedApi;

    #[async_trait]
    impl FeishuApprovalApi for UnusedApi {
        async fn get_instance(&self, _instance_code: &str) -> Result<ApprovalInstance> {
            unreachable!()
        }

        async fn get_applicant(
            &self,
            _user_id: &str,
            _user_id_type: &str,
        ) -> Result<ApprovalApplicant> {
            unreachable!()
        }
    }

    fn encrypt(key_text: &str, plain: &[u8]) -> String {
        type Encryptor = cbc::Encryptor<Aes256>;
        let key: [u8; 32] = Sha256::digest(key_text.as_bytes()).into();
        let iv = [7_u8; 16];
        let mut buffer = vec![0_u8; plain.len() + 16];
        buffer[..plain.len()].copy_from_slice(plain);
        let encrypted = Encryptor::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, plain.len())
            .unwrap();
        let mut combined = iv.to_vec();
        combined.extend_from_slice(encrypted);
        STANDARD.encode(combined)
    }

    #[test]
    fn signature_and_aes_roundtrip_match_lark_contract() {
        let key_text = "approval-encrypt-key";
        let plain = br#"{"token":"verify","challenge":"ok"}"#;
        assert_eq!(
            decrypt_event(key_text, &encrypt(key_text, plain)).unwrap(),
            plain
        );

        let body = br#"{"encrypt":"cipher"}"#;
        let timestamp = "100";
        let nonce = "nonce";
        let mut digest = Sha256::new();
        digest.update(timestamp);
        digest.update(nonce);
        digest.update(key_text);
        digest.update(body);
        let signature = hex_bytes(&digest.finalize());
        let headers = ApprovalEventHeaders {
            timestamp: Some(timestamp),
            nonce: Some(nonce),
            signature: Some(&signature),
        };
        verify_signature(&headers, key_text, body).unwrap();
        assert!(verify_signature(&headers, key_text, b"changed").is_err());
    }

    #[test]
    fn form_uses_exact_control_ids_and_multiple_groups() {
        let form = json!([
            {"id":"group-control","value":[{"id":"group-42","text":"中文文案不参与授权"}]},
            {"widgetId":"expiry-control","value":"2026-07-31"},
            {"control_id":"reason-control","value":{"text":"project access"}}
        ]);
        let fields = parse_form(
            &Value::String(form.to_string()),
            "group-control",
            "expiry-control",
            "reason-control",
        )
        .unwrap();
        assert_eq!(fields.group_ids, ["group-42"]);
        assert_eq!(fields.reason, "project access");
        let expected = FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(fields.expires_at, expected);

        let mut multiple = form;
        multiple[0]["value"] = json!([{"id":"g1"},{"id":"g2"}]);
        assert_eq!(
            parse_form(
                &multiple,
                "group-control",
                "expiry-control",
                "reason-control"
            )
            .unwrap()
            .group_ids,
            ["g1", "g2"]
        );
        assert_eq!(
            option_ids(&json!(["g2", "g1", "g2"])).unwrap(),
            ["g1", "g2"]
        );
        assert_eq!(option_ids(&json!("[\"g1\",\"g2\"]")).unwrap(), ["g1", "g2"]);
        assert!(option_ids(&json!([])).is_err());
        assert!(option_ids(&json!(["g1", ""])).is_err());
        assert!(option_ids(&json!([{"text":"display label only"}])).is_err());
        assert!(parse_form(&multiple, "网络组", "expiry-control", "reason-control").is_err());
    }

    #[test]
    fn instance_selected_options_take_precedence_over_display_names() {
        let form = json!([
            {"id":"group","type":"checkboxV2","value":["开发", "本地开发"],
             "option":[{"key":"g1","text":"开发"},{"key":"g2","text":"本地开发"}]},
            {"id":"expiry","type":"date","value":"2026-09-14T00:00:00+08:00"},
            {"id":"reason","value":"access"}
        ]);
        assert_eq!(
            parse_form(&form, "group", "expiry", "reason")
                .unwrap()
                .group_ids,
            ["g1", "g2"]
        );
        assert_eq!(
            group_option_ids(&json!({"value":"显示名称", "option":{"key":"g1","text":"显示名称"}}))
                .unwrap(),
            ["g1"]
        );
        // 选项损坏时不能回退到碰巧等于用户组 ID 的文案。
        assert!(group_option_ids(&json!({"value":["g1"],"option":[{"text":"g1"}]})).is_err());
        assert!(group_option_ids(&json!({"value":["g1"],"option":[]})).is_err());
    }

    #[test]
    fn epoch_date_is_interpreted_in_shanghai_before_exclusive_boundary() {
        // 2026-07-31 16:30 UTC = 2026-08-01 00:30 上海；独占边界为 8/2 00:00 上海。
        let value = Value::String("1785515400000".into());
        let expected = FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 2, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(exclusive_expiry(&value).unwrap(), expected);
        assert_eq!(
            exclusive_expiry(&json!({"value": value})).unwrap(),
            expected
        );
        assert!(exclusive_expiry(&Value::String("2026-07-31garbage".into())).is_err());
    }

    #[test]
    fn event_meta_supports_v2_event_id_and_legacy_uuid() {
        let v2 = json!({
            "header":{"event_id":"evt-1"},
            "event":{"approval_code":"code","instance_code":"instance","status":"APPROVED"}
        });
        assert_eq!(parse_event_meta(&v2).unwrap().event_id, "evt-1");
        let legacy = json!({
            "uuid":"old-1","approval_code":"code",
            "instance_code":"instance","status":"APPROVED"
        });
        assert_eq!(parse_event_meta(&legacy).unwrap().event_id, "old-1");
    }

    #[test]
    fn idempotency_hash_ignores_delivery_ids_but_detects_payload_changes() {
        let first = json!({
            "header":{"event_id":"evt-1","create_time":"100"},
            "event":{"instance_code":"instance-1","status":"APPROVED","extra":"same"}
        });
        let replay = json!({
            "header":{"event_id":"evt-2","create_time":"200"},
            "event":{"instance_code":"instance-1","status":"APPROVED","extra":"same"}
        });
        let changed = json!({
            "header":{"event_id":"evt-2","create_time":"200"},
            "event":{"instance_code":"instance-1","status":"APPROVED","extra":"changed"}
        });
        let first_hash = idempotency_payload_hash(first).unwrap();
        let replay_hash = idempotency_payload_hash(replay).unwrap();
        let changed_hash = idempotency_payload_hash(changed).unwrap();
        assert_eq!(first_hash, replay_hash);
        assert_ne!(replay_hash, changed_hash);
    }

    #[test]
    fn upstream_configuration_errors_are_retried_but_invalid_payloads_are_rejected() {
        assert!(!permanent(&AppError::Config("upstream".into())));
        assert!(permanent(&AppError::Validation("payload".into())));
    }

    #[tokio::test]
    async fn receive_validates_signature_decryption_and_token_before_inbox() {
        let url = format!(
            "sqlite:file:approval_receive_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let config = FeishuApprovalConfig {
            approval_code: Some("approval-code".into()),
            group_control_id: Some("group".into()),
            expiry_control_id: Some("expiry".into()),
            reason_control_id: Some("reason".into()),
            verification_token: Some("verification-token".into()),
            encrypt_key: Some("encrypt-key".into()),
        };
        let service = FeishuApprovalService::new(
            config,
            SqliteAccessGrantRepository::new(pool.clone()),
            Arc::new(UnusedApi),
            Arc::new(crate::services::Argon2Hasher::new()),
        );
        let event = json!({
            "token":"verification-token",
            "header":{"event_id":"evt-1"},
            "event":{"approval_code":"approval-code","instance_code":"instance-1","status":"APPROVED"}
        });
        let body = serde_json::to_vec(&json!({
            "encrypt": encrypt("encrypt-key", &serde_json::to_vec(&event).unwrap())
        }))
        .unwrap();
        let timestamp = Utc::now().timestamp().to_string();
        let nonce = "nonce";
        let mut digest = Sha256::new();
        digest.update(timestamp.as_bytes());
        digest.update(nonce.as_bytes());
        digest.update(b"encrypt-key");
        digest.update(&body);
        let signature = hex_bytes(&digest.finalize());
        assert_eq!(
            service
                .receive(
                    ApprovalEventHeaders {
                        timestamp: Some(&timestamp),
                        nonce: Some(nonce),
                        signature: Some(&signature),
                    },
                    &body,
                )
                .await
                .unwrap(),
            ApprovalWebhookReply::Ack
        );
        let saved: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM feishu_approval_inbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(saved.0, 1);

        let replay = json!({
            "token":"verification-token",
            "header":{"event_id":"evt-2"},
            "event":{"approval_code":"approval-code","instance_code":"instance-1","status":"APPROVED"}
        });
        let replay_body = serde_json::to_vec(&json!({
            "encrypt": encrypt("encrypt-key", &serde_json::to_vec(&replay).unwrap())
        }))
        .unwrap();
        let mut replay_digest = Sha256::new();
        replay_digest.update(timestamp.as_bytes());
        replay_digest.update(nonce.as_bytes());
        replay_digest.update(b"encrypt-key");
        replay_digest.update(&replay_body);
        let replay_signature = hex_bytes(&replay_digest.finalize());
        assert_eq!(
            service
                .receive(
                    ApprovalEventHeaders {
                        timestamp: Some(&timestamp),
                        nonce: Some(nonce),
                        signature: Some(&replay_signature),
                    },
                    &replay_body,
                )
                .await
                .unwrap(),
            ApprovalWebhookReply::Ack
        );
        let replay_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM feishu_approval_inbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(replay_count.0, 1);
        assert!(service
            .receive(
                ApprovalEventHeaders {
                    timestamp: Some(&timestamp),
                    nonce: Some(nonce),
                    signature: Some("bad"),
                },
                &body,
            )
            .await
            .is_err());

        assert!(service
            .receive(
                ApprovalEventHeaders {
                    timestamp: None,
                    nonce: None,
                    signature: None,
                },
                &body,
            )
            .await
            .is_err());

        let challenge = json!({
            "token":"verification-token",
            "challenge":"challenge-value",
            "type":"url_verification"
        });
        let challenge_body = serde_json::to_vec(&json!({
            "encrypt": encrypt("encrypt-key", &serde_json::to_vec(&challenge).unwrap())
        }))
        .unwrap();
        assert_eq!(
            service
                .receive(
                    ApprovalEventHeaders {
                        timestamp: None,
                        nonce: None,
                        signature: None,
                    },
                    &challenge_body,
                )
                .await
                .unwrap(),
            ApprovalWebhookReply::Challenge("challenge-value".into())
        );

        let not_challenge = json!({
            "token":"verification-token",
            "challenge":"must-not-bypass-signature",
            "type":"approval_instance"
        });
        let not_challenge_body = serde_json::to_vec(&json!({
            "encrypt": encrypt("encrypt-key", &serde_json::to_vec(&not_challenge).unwrap())
        }))
        .unwrap();
        assert!(service
            .receive(
                ApprovalEventHeaders {
                    timestamp: None,
                    nonce: None,
                    signature: None,
                },
                &not_challenge_body,
            )
            .await
            .is_err());
    }
}
