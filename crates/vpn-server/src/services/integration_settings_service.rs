//! 管理后台飞书集成配置：单键持久化、秘密三态更新与重启边界。

use std::sync::Arc;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use vpn_api_types::system::{
    FeishuApprovalSettingsView, FeishuApprovalSubscriptionView, FeishuExternalOptionsSettingsView,
    FeishuLoginSettingsView, IntegrationSettingsSnapshotView, IntegrationSettingsView,
    SecretUpdate, UpdateIntegrationSettingsRequest,
};
use vpn_core::{AppError, Result};

use crate::{
    config::{FeishuApprovalConfig, FeishuApprovalOptionsConfig, FeishuConfig},
    repositories::SqliteSystemConfigRepository,
};

const SETTINGS_KEY: &str = "integration_settings_v1";
const SETTINGS_VERSION: u8 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredIntegrationSettings {
    version: u8,
    feishu_login: StoredFeishuLogin,
    feishu_approval: StoredFeishuApproval,
    external_options: StoredExternalOptions,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredFeishuLogin {
    enabled: bool,
    app_id: Option<String>,
    app_secret: Option<String>,
    redirect_uri: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredFeishuApproval {
    enabled: bool,
    approval_code: Option<String>,
    group_control_id: Option<String>,
    expiry_control_id: Option<String>,
    reason_control_id: Option<String>,
    max_devices_control_id: Option<String>,
    verification_token: Option<String>,
    encrypt_key: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredExternalOptions {
    token: Option<String>,
}

/// 启动后 `applied` 固定不变；页面保存只更新 DB 中的 `desired`。
#[derive(Clone)]
pub struct IntegrationSettingsService {
    repo: SqliteSystemConfigRepository,
    pool: SqlitePool,
    applied: Arc<StoredIntegrationSettings>,
    update_lock: Arc<Mutex<()>>,
}

impl IntegrationSettingsService {
    fn subscription_key(&self) -> Option<String> {
        let identity = serde_json::to_vec(&(
            self.applied.feishu_login.app_id.as_ref()?,
            self.applied.feishu_approval.approval_code.as_ref()?,
        ))
        .ok()?;
        Some(format!(
            "feishu_approval_subscription_v1_{:x}",
            Sha256::digest(identity)
        ))
    }

    pub async fn approval_subscription(&self) -> Result<FeishuApprovalSubscriptionView> {
        let desired = self.desired().await?;
        let last_success_at = match self.subscription_key() {
            Some(key) => self
                .repo
                .get(&key)
                .await?
                .map(|value| value.parse::<i64>())
                .transpose()
                .map_err(|_| AppError::Config("飞书订阅记录格式异常".into()))?,
            None => None,
        };
        Ok(FeishuApprovalSubscriptionView {
            app_id: self.applied.feishu_login.app_id.clone(),
            approval_code: self.applied.feishu_approval.approval_code.clone(),
            last_success_at,
            can_subscribe: desired == *self.applied
                && self.applied.feishu_login.enabled
                && self.applied.feishu_approval.enabled,
        })
    }

    pub async fn subscribe_approval(&self) -> Result<FeishuApprovalSubscriptionView> {
        self.subscribe_approval_with(|config, code| async move {
            super::feishu_approval_service::ReqwestFeishuApprovalApi::new(config)?
                .subscribe(&code)
                .await
        })
        .await
    }

    async fn subscribe_approval_with<F, Fut>(
        &self,
        subscribe: F,
    ) -> Result<FeishuApprovalSubscriptionView>
    where
        F: FnOnce(FeishuConfig, String) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let _guard = self.update_lock.try_lock().map_err(|_| {
            AppError::Validation("集成设置正在更新或订阅正在执行，请稍后重试".into())
        })?;
        let status = self.approval_subscription().await?;
        if !status.can_subscribe {
            return Err(AppError::Validation(
                "请启用并完整保存飞书登录与审批配置，重启生效后再订阅".into(),
            ));
        }
        let code = status
            .approval_code
            .ok_or_else(|| AppError::Config("飞书审批定义未配置".into()))?;
        let key = self
            .subscription_key()
            .ok_or_else(|| AppError::Config("飞书应用未配置".into()))?;
        // 记录仅表示历史成功；即使有记录也再次请求，以支持远端取消后的恢复。
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            subscribe(self.applied.runtime().0, code),
        )
        .await
        .map_err(|_| AppError::Config("飞书订阅请求超时，请重试".into()))??;
        self.repo
            .set(&key, &chrono::Utc::now().timestamp_millis().to_string())
            .await?;
        self.approval_subscription().await
    }

    pub async fn load_or_seed(
        repo: SqliteSystemConfigRepository,
        pool: SqlitePool,
        feishu: &FeishuConfig,
        approval: &FeishuApprovalConfig,
        options: &FeishuApprovalOptionsConfig,
    ) -> anyhow::Result<Self> {
        let stored = if let Some(stored) = repo.get(SETTINGS_KEY).await? {
            // 聚合配置一旦存在，环境变量彻底退出控制面；包括已变成非法的旧值。
            stored
        } else {
            let seed = StoredIntegrationSettings::from_environment(feishu, approval, options);
            seed.validate().map_err(anyhow::Error::msg)?;
            let serialized = serialize_settings(&seed).map_err(anyhow::Error::msg)?;
            repo.set_if_absent(SETTINGS_KEY, &serialized).await?;
            repo.get(SETTINGS_KEY)
                .await?
                .ok_or_else(|| anyhow::anyhow!("集成设置初始化后仍不存在"))?
        };
        let applied = parse_settings(&stored).map_err(anyhow::Error::msg)?;
        applied.validate().map_err(anyhow::Error::msg)?;
        ensure_disabled_approval_is_safe(&pool, &applied)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(Self {
            repo,
            pool,
            applied: Arc::new(applied),
            update_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn applied_runtime(
        &self,
    ) -> (
        FeishuConfig,
        FeishuApprovalConfig,
        FeishuApprovalOptionsConfig,
    ) {
        self.applied.runtime()
    }

    pub async fn view(&self) -> Result<IntegrationSettingsView> {
        let desired = self.desired().await?;
        Ok(IntegrationSettingsView {
            applied: self.applied.public_view(),
            desired: desired.public_view(),
            restart_required: desired != *self.applied,
        })
    }

    pub async fn update(
        &self,
        request: UpdateIntegrationSettingsRequest,
        applied_wg_backend: &str,
    ) -> Result<IntegrationSettingsView> {
        let _guard = self.update_lock.lock().await;
        let current = self.desired().await?;
        let next = current.apply(request)?;
        next.validate().map_err(AppError::Validation)?;
        if next.feishu_approval.enabled && applied_wg_backend != "kernel" {
            return Err(AppError::Validation(
                "启用飞书审批要求当前已应用的 wg_backend=kernel".to_string(),
            ));
        }

        let serialized = serialize_settings(&next).map_err(AppError::Validation)?;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| AppError::Database(Box::new(error)))?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|error| AppError::Database(Box::new(error)))?;
        if !next.feishu_approval.enabled {
            let count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM users WHERE access_mode = 'approval_required'",
            )
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| AppError::Database(Box::new(error)))?;
            if count.0 > 0 {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                return Err(AppError::Validation(format!(
                    "仍有 {} 个审批管控用户，迁移后才能关闭飞书审批",
                    count.0
                )));
            }
        }
        let now = chrono::Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"INSERT INTO system_config (key, value, updated_at)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at"#,
        )
        .bind(SETTINGS_KEY)
        .bind(&serialized)
        .bind(now)
        .execute(&mut *connection)
        .await;
        if let Err(error) = result {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            return Err(AppError::Database(Box::new(error)));
        }
        sqlx::query("COMMIT")
            .execute(&mut *connection)
            .await
            .map_err(|error| AppError::Database(Box::new(error)))?;
        Ok(IntegrationSettingsView {
            applied: self.applied.public_view(),
            desired: next.public_view(),
            restart_required: next != *self.applied,
        })
    }

    async fn desired(&self) -> Result<StoredIntegrationSettings> {
        let raw = self
            .repo
            .get(SETTINGS_KEY)
            .await?
            .ok_or_else(|| AppError::Config("集成设置不存在".to_string()))?;
        let settings = parse_settings(&raw).map_err(AppError::Config)?;
        settings.validate().map_err(AppError::Config)?;
        Ok(settings)
    }
}

impl StoredIntegrationSettings {
    fn from_environment(
        feishu: &FeishuConfig,
        approval: &FeishuApprovalConfig,
        options: &FeishuApprovalOptionsConfig,
    ) -> Self {
        Self {
            version: SETTINGS_VERSION,
            feishu_login: StoredFeishuLogin {
                enabled: feishu.enabled(),
                app_id: clean(feishu.app_id.clone()),
                app_secret: clean(feishu.app_secret.clone()),
                redirect_uri: clean(feishu.redirect_uri.clone()),
            },
            feishu_approval: StoredFeishuApproval {
                enabled: approval.enabled(),
                approval_code: clean(approval.approval_code.clone()),
                group_control_id: clean(approval.group_control_id.clone()),
                expiry_control_id: clean(approval.expiry_control_id.clone()),
                reason_control_id: clean(approval.reason_control_id.clone()),
                max_devices_control_id: clean(approval.max_devices_control_id.clone()),
                verification_token: clean(approval.verification_token.clone()),
                encrypt_key: clean(approval.encrypt_key.clone()),
            },
            external_options: StoredExternalOptions {
                token: clean(options.token.clone()),
            },
        }
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if self.version != SETTINGS_VERSION {
            return Err(format!("不支持的集成设置版本：{}", self.version));
        }
        for (name, value) in [
            ("飞书登录 App ID", self.feishu_login.app_id.as_deref()),
            (
                "飞书登录 App Secret",
                self.feishu_login.app_secret.as_deref(),
            ),
            (
                "飞书登录回调地址",
                self.feishu_login.redirect_uri.as_deref(),
            ),
            (
                "飞书审批 Code",
                self.feishu_approval.approval_code.as_deref(),
            ),
            (
                "用户组控件 ID",
                self.feishu_approval.group_control_id.as_deref(),
            ),
            (
                "到期日控件 ID",
                self.feishu_approval.expiry_control_id.as_deref(),
            ),
            (
                "原因控件 ID",
                self.feishu_approval.reason_control_id.as_deref(),
            ),
            (
                "终端上限控件 ID",
                self.feishu_approval.max_devices_control_id.as_deref(),
            ),
            (
                "Verification Token",
                self.feishu_approval.verification_token.as_deref(),
            ),
            ("Encrypt Key", self.feishu_approval.encrypt_key.as_deref()),
            ("外部选项 Token", self.external_options.token.as_deref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(format!("{name} 不能是空白字符串"));
            }
        }
        let login_values = [
            self.feishu_login.app_id.as_deref(),
            self.feishu_login.app_secret.as_deref(),
            self.feishu_login.redirect_uri.as_deref(),
        ];
        if login_values.iter().any(|value| value.is_some())
            && !login_values.iter().all(|value| value.is_some())
        {
            return Err("飞书登录 App ID、App Secret 和回调地址必须同时配置".to_string());
        }
        if self.feishu_login.enabled && !login_values.iter().all(|value| value.is_some()) {
            return Err("启用飞书登录前必须完整配置 App ID、App Secret 和回调地址".to_string());
        }
        if let Some(uri) = &self.feishu_login.redirect_uri {
            validate_redirect_uri(uri)?;
        }

        let approval_values = [
            self.feishu_approval.approval_code.as_deref(),
            self.feishu_approval.group_control_id.as_deref(),
            self.feishu_approval.expiry_control_id.as_deref(),
            self.feishu_approval.reason_control_id.as_deref(),
            self.feishu_approval.verification_token.as_deref(),
            self.feishu_approval.encrypt_key.as_deref(),
        ];
        if approval_values.iter().any(|value| value.is_some())
            && !approval_values.iter().all(|value| value.is_some())
        {
            return Err("飞书审批六项配置必须同时设置".to_string());
        }
        if self.feishu_approval.enabled {
            if !self.feishu_login.enabled {
                return Err("飞书审批依赖已启用的飞书登录".to_string());
            }
            if !approval_values.iter().all(|value| value.is_some()) {
                return Err("启用飞书审批前必须完整配置全部审批字段".to_string());
            }
        }
        if self
            .feishu_approval
            .verification_token
            .as_deref()
            .is_some_and(|value| value.chars().count() < 16)
            || self
                .feishu_approval
                .encrypt_key
                .as_deref()
                .is_some_and(|value| value.chars().count() < 16)
        {
            return Err("飞书审批 Verification Token 与 Encrypt Key 至少需要 16 个字符".into());
        }
        if self
            .external_options
            .token
            .as_deref()
            .is_some_and(|value| value.chars().count() < 32)
        {
            return Err("飞书审批外部选项 Token 至少需要 32 个字符".into());
        }
        Ok(())
    }

    fn apply(self, request: UpdateIntegrationSettingsRequest) -> Result<Self> {
        Ok(Self {
            version: SETTINGS_VERSION,
            feishu_login: StoredFeishuLogin {
                enabled: request.feishu_login.enabled,
                app_id: clean(request.feishu_login.app_id),
                app_secret: apply_secret(
                    self.feishu_login.app_secret,
                    request.feishu_login.app_secret,
                    "App Secret",
                )?,
                redirect_uri: clean(request.feishu_login.redirect_uri),
            },
            feishu_approval: StoredFeishuApproval {
                enabled: request.feishu_approval.enabled,
                approval_code: clean(request.feishu_approval.approval_code),
                group_control_id: clean(request.feishu_approval.group_control_id),
                expiry_control_id: clean(request.feishu_approval.expiry_control_id),
                reason_control_id: clean(request.feishu_approval.reason_control_id),
                max_devices_control_id: clean(request.feishu_approval.max_devices_control_id),
                verification_token: apply_secret(
                    self.feishu_approval.verification_token,
                    request.feishu_approval.verification_token,
                    "Verification Token",
                )?,
                encrypt_key: apply_secret(
                    self.feishu_approval.encrypt_key,
                    request.feishu_approval.encrypt_key,
                    "Encrypt Key",
                )?,
            },
            external_options: StoredExternalOptions {
                token: apply_secret(
                    self.external_options.token,
                    request.external_options.token,
                    "外部选项 Token",
                )?,
            },
        })
    }

    fn runtime(
        &self,
    ) -> (
        FeishuConfig,
        FeishuApprovalConfig,
        FeishuApprovalOptionsConfig,
    ) {
        let feishu = if self.feishu_login.enabled {
            FeishuConfig {
                app_id: self.feishu_login.app_id.clone(),
                app_secret: self.feishu_login.app_secret.clone(),
                redirect_uri: self.feishu_login.redirect_uri.clone(),
            }
        } else {
            FeishuConfig::default()
        };
        let approval = if self.feishu_approval.enabled {
            FeishuApprovalConfig {
                approval_code: self.feishu_approval.approval_code.clone(),
                group_control_id: self.feishu_approval.group_control_id.clone(),
                expiry_control_id: self.feishu_approval.expiry_control_id.clone(),
                reason_control_id: self.feishu_approval.reason_control_id.clone(),
                max_devices_control_id: self.feishu_approval.max_devices_control_id.clone(),
                verification_token: self.feishu_approval.verification_token.clone(),
                encrypt_key: self.feishu_approval.encrypt_key.clone(),
            }
        } else {
            // 通讯录回调与审批共用事件凭据，停用审批不应清空凭据。
            FeishuApprovalConfig {
                verification_token: self.feishu_approval.verification_token.clone(),
                encrypt_key: self.feishu_approval.encrypt_key.clone(),
                ..FeishuApprovalConfig::default()
            }
        };
        let options = FeishuApprovalOptionsConfig {
            token: self.external_options.token.clone(),
        };
        (feishu, approval, options)
    }

    fn public_view(&self) -> IntegrationSettingsSnapshotView {
        IntegrationSettingsSnapshotView {
            feishu_login: FeishuLoginSettingsView {
                enabled: self.feishu_login.enabled,
                app_id: self.feishu_login.app_id.clone(),
                redirect_uri: self.feishu_login.redirect_uri.clone(),
                app_secret_set: self.feishu_login.app_secret.is_some(),
            },
            feishu_approval: FeishuApprovalSettingsView {
                enabled: self.feishu_approval.enabled,
                approval_code: self.feishu_approval.approval_code.clone(),
                group_control_id: self.feishu_approval.group_control_id.clone(),
                expiry_control_id: self.feishu_approval.expiry_control_id.clone(),
                reason_control_id: self.feishu_approval.reason_control_id.clone(),
                max_devices_control_id: self.feishu_approval.max_devices_control_id.clone(),
                verification_token_set: self.feishu_approval.verification_token.is_some(),
                encrypt_key_set: self.feishu_approval.encrypt_key.is_some(),
            },
            external_options: FeishuExternalOptionsSettingsView {
                token_set: self.external_options.token.is_some(),
            },
        }
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn apply_secret(
    current: Option<String>,
    update: SecretUpdate,
    name: &str,
) -> Result<Option<String>> {
    let replacement = clean(update.value);
    if update.clear && replacement.is_some() {
        return Err(AppError::Validation(format!("{name} 不能同时替换和清除")));
    }
    if update.clear {
        Ok(None)
    } else {
        Ok(replacement.or(current))
    }
}

fn validate_redirect_uri(raw: &str) -> std::result::Result<(), String> {
    let uri = Url::parse(raw).map_err(|_| "飞书回调地址必须是合法 HTTPS URL".to_string())?;
    if uri.scheme() != "https"
        || uri.host_str().is_none()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
        || uri.path() != "/api/v1/auth/feishu/callback"
    {
        return Err(
            "飞书回调地址必须使用 HTTPS、不得包含凭证/query/fragment，且路径必须为 /api/v1/auth/feishu/callback"
                .to_string(),
        );
    }
    Ok(())
}

fn serialize_settings(settings: &StoredIntegrationSettings) -> std::result::Result<String, String> {
    serde_json::to_string(settings).map_err(|error| format!("序列化集成设置失败：{error}"))
}

fn parse_settings(raw: &str) -> std::result::Result<StoredIntegrationSettings, String> {
    serde_json::from_str(raw).map_err(|error| format!("解析集成设置失败：{error}"))
}

async fn ensure_disabled_approval_is_safe(
    pool: &SqlitePool,
    settings: &StoredIntegrationSettings,
) -> Result<()> {
    if settings.feishu_approval.enabled {
        return Ok(());
    }
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE access_mode = 'approval_required'")
            .fetch_one(pool)
            .await
            .map_err(|error| AppError::Database(Box::new(error)))?;
    if count.0 > 0 {
        return Err(AppError::Config(format!(
            "飞书审批已关闭但仍有 {} 个 approval_required 用户，拒绝启动以避免取消 ACL 后意外放权",
            count.0
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    #[test]
    fn contact_event_credentials_survive_disabled_approval() {
        let stored = StoredIntegrationSettings::from_environment(
            &FeishuConfig::default(),
            &FeishuApprovalConfig {
                verification_token: Some("verification-token".into()),
                encrypt_key: Some("encrypt-key".into()),
                ..Default::default()
            },
            &FeishuApprovalOptionsConfig::default(),
        );
        let (_, approval, _) = stored.runtime();
        assert!(!approval.enabled());
        assert_eq!(
            approval.verification_token.as_deref(),
            Some("verification-token")
        );
        assert_eq!(approval.encrypt_key.as_deref(), Some("encrypt-key"));
    }

    async fn setup() -> (IntegrationSettingsService, SqliteSystemConfigRepository) {
        let url = format!(
            "sqlite:file:integration_settings_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let options = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let repo = SqliteSystemConfigRepository::new(pool.clone());
        let service = IntegrationSettingsService::load_or_seed(
            repo.clone(),
            pool,
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        )
        .await
        .unwrap();
        (service, repo)
    }

    fn complete_request(secret: &str) -> UpdateIntegrationSettingsRequest {
        UpdateIntegrationSettingsRequest {
            feishu_login: vpn_api_types::system::UpdateFeishuLoginSettings {
                enabled: true,
                app_id: Some("cli_test".into()),
                redirect_uri: Some("https://vpn.example.com/api/v1/auth/feishu/callback".into()),
                app_secret: SecretUpdate {
                    value: Some(secret.into()),
                    clear: false,
                },
            },
            feishu_approval: vpn_api_types::system::UpdateFeishuApprovalSettings {
                enabled: false,
                approval_code: None,
                group_control_id: None,
                expiry_control_id: None,
                reason_control_id: None,
                max_devices_control_id: None,
                verification_token: SecretUpdate::default(),
                encrypt_key: SecretUpdate::default(),
            },
            external_options: vpn_api_types::system::UpdateFeishuExternalOptionsSettings {
                token: SecretUpdate::default(),
            },
        }
    }

    fn approval_request() -> UpdateIntegrationSettingsRequest {
        let mut request = complete_request("app-secret");
        request.feishu_approval = vpn_api_types::system::UpdateFeishuApprovalSettings {
            enabled: true,
            approval_code: Some("code".into()),
            group_control_id: Some("group".into()),
            expiry_control_id: Some("expiry".into()),
            reason_control_id: Some("reason".into()),
            max_devices_control_id: None,
            verification_token: SecretUpdate {
                value: Some("1234567890123456".into()),
                clear: false,
            },
            encrypt_key: SecretUpdate {
                value: Some("abcdefghijklmnop".into()),
                clear: false,
            },
        };
        request
    }

    async fn restart(service: &IntegrationSettingsService) -> IntegrationSettingsService {
        IntegrationSettingsService::load_or_seed(
            service.repo.clone(),
            service.pool.clone(),
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn optional_device_control_survives_save_restart_and_clear() {
        let old = serde_json::json!({
            "enabled": false, "approval_code": null, "group_control_id": null,
            "expiry_control_id": null, "reason_control_id": null,
            "verification_token": null, "encrypt_key": null
        });
        let stored: StoredFeishuApproval = serde_json::from_value(old).unwrap();
        assert!(stored.max_devices_control_id.is_none());
        let (service, _) = setup().await;
        let mut request = approval_request();
        request.feishu_approval.max_devices_control_id = Some(" devices ".into());
        let view = service.update(request, "kernel").await.unwrap();
        assert!(view.restart_required);
        assert_eq!(
            view.desired
                .feishu_approval
                .max_devices_control_id
                .as_deref(),
            Some("devices")
        );
        let service = restart(&service).await;
        assert_eq!(
            service
                .applied_runtime()
                .1
                .max_devices_control_id
                .as_deref(),
            Some("devices")
        );
        service.update(approval_request(), "kernel").await.unwrap();
        let service = restart(&service).await;
        assert!(service.applied_runtime().1.max_devices_control_id.is_none());
    }

    #[tokio::test]
    async fn subscription_requires_applied_config_and_persists_success_per_identity() {
        let (service, _) = setup().await;
        assert!(!service.approval_subscription().await.unwrap().can_subscribe);
        service.update(approval_request(), "kernel").await.unwrap();
        assert!(service
            .subscribe_approval_with(|_, _| async { panic!("pending config must not call API") })
            .await
            .is_err());
        let service = restart(&service).await;
        let status = service
            .subscribe_approval_with(|config, code| async move {
                assert_eq!(config.app_id.as_deref(), Some("cli_test"));
                assert_eq!(code, "code");
                Ok(())
            })
            .await
            .unwrap();
        assert!(status.last_success_at.is_some());
        let service = restart(&service).await;
        assert_eq!(service.approval_subscription().await.unwrap(), status);
        // Failure after prior success preserves historical success, but is returned to caller.
        assert!(service
            .subscribe_approval_with(|_, _| async { Err(AppError::Config("mock failure".into())) })
            .await
            .is_err());
        assert_eq!(
            service
                .approval_subscription()
                .await
                .unwrap()
                .last_success_at,
            status.last_success_at
        );
        let mut changed = approval_request();
        changed.feishu_approval.approval_code = Some("another-code".into());
        service.update(changed, "kernel").await.unwrap();
        assert!(!service.approval_subscription().await.unwrap().can_subscribe);
        let service = restart(&service).await;
        assert_eq!(
            service
                .approval_subscription()
                .await
                .unwrap()
                .last_success_at,
            None
        );
        assert!(service
            .subscribe_approval_with(|_, _| async { Err(AppError::Config("mock failure".into())) })
            .await
            .is_err());
        assert_eq!(
            service
                .approval_subscription()
                .await
                .unwrap()
                .last_success_at,
            None
        );
        service.update(approval_request(), "kernel").await.unwrap();
        let service = restart(&service).await;
        assert_eq!(
            service
                .approval_subscription()
                .await
                .unwrap()
                .last_success_at,
            status.last_success_at
        );
        // Same definition under another application must have its own record.
        let mut changed = approval_request();
        changed.feishu_login.app_id = Some("cli_other".into());
        service.update(changed, "kernel").await.unwrap();
        let service = restart(&service).await;
        assert_eq!(
            service
                .approval_subscription()
                .await
                .unwrap()
                .last_success_at,
            None
        );
    }

    #[tokio::test]
    async fn secret_is_never_exposed_and_blank_update_keeps_it() {
        let (service, repo) = setup().await;
        let view = service
            .update(complete_request("top-secret"), "kernel")
            .await
            .unwrap();
        assert!(view.desired.feishu_login.app_secret_set);
        assert!(!serde_json::to_string(&view).unwrap().contains("top-secret"));

        let mut keep = complete_request("");
        keep.feishu_login.app_secret.value = None;
        service.update(keep, "kernel").await.unwrap();
        assert!(repo
            .get(SETTINGS_KEY)
            .await
            .unwrap()
            .unwrap()
            .contains("top-secret"));
    }

    #[tokio::test]
    async fn invalid_update_is_atomic_and_strict_redirect_is_enforced() {
        let (service, repo) = setup().await;
        service
            .update(complete_request("old-secret"), "kernel")
            .await
            .unwrap();
        let before = repo.get(SETTINGS_KEY).await.unwrap().unwrap();
        let mut invalid = complete_request("new-secret");
        invalid.feishu_login.redirect_uri =
            Some("https://u:p@vpn.example.com/api/v1/auth/feishu/callback?q=1".into());
        assert!(service.update(invalid, "kernel").await.is_err());
        assert_eq!(repo.get(SETTINGS_KEY).await.unwrap().unwrap(), before);
    }

    #[tokio::test]
    async fn disabling_approval_with_managed_users_is_rejected() {
        let (service, _) = setup().await;
        sqlx::query(
            "INSERT INTO users (id,username,email,password_hash,role,status,must_change_password,created_at,updated_at,access_mode) VALUES ('u','u','u@example.com','h','user','active',0,0,0,'approval_required')",
        )
        .execute(&service.pool)
        .await
        .unwrap();
        // 当前 desired 未启用时不触发“关闭”保护；先直接写入一个完整启用快照。
        let mut enabled = StoredIntegrationSettings::from_environment(
            &FeishuConfig {
                app_id: Some("id".into()),
                app_secret: Some("secret".into()),
                redirect_uri: Some("https://vpn.example.com/api/v1/auth/feishu/callback".into()),
            },
            &FeishuApprovalConfig {
                approval_code: Some("code".into()),
                group_control_id: Some("group".into()),
                expiry_control_id: Some("expiry".into()),
                reason_control_id: Some("reason".into()),
                max_devices_control_id: None,
                verification_token: Some("1234567890123456".into()),
                encrypt_key: Some("abcdefghijklmnop".into()),
            },
            &FeishuApprovalOptionsConfig::default(),
        );
        enabled.feishu_approval.enabled = true;
        service
            .repo
            .set(SETTINGS_KEY, &serialize_settings(&enabled).unwrap())
            .await
            .unwrap();
        let mut request = complete_request("");
        request.feishu_approval.approval_code = Some("code".into());
        request.feishu_approval.group_control_id = Some("group".into());
        request.feishu_approval.expiry_control_id = Some("expiry".into());
        request.feishu_approval.reason_control_id = Some("reason".into());
        let error = service.update(request, "kernel").await.unwrap_err();
        assert!(error.to_string().contains("审批管控用户"));
    }

    #[tokio::test]
    async fn existing_database_value_ignores_changed_environment_seed() {
        let (service, repo) = setup().await;
        service
            .update(complete_request("database-secret"), "kernel")
            .await
            .unwrap();
        let reloaded = IntegrationSettingsService::load_or_seed(
            repo,
            service.pool.clone(),
            &FeishuConfig {
                app_id: Some("incomplete-new-env".into()),
                app_secret: None,
                redirect_uri: None,
            },
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig {
                token: Some("short".into()),
            },
        )
        .await
        .unwrap();
        let view = reloaded.view().await.unwrap();
        assert_eq!(
            view.desired.feishu_login.app_id.as_deref(),
            Some("cli_test")
        );
        assert!(view.desired.feishu_login.app_secret_set);
    }

    #[tokio::test]
    async fn clear_conflict_and_token_boundaries_are_enforced() {
        let (service, _) = setup().await;
        service
            .update(complete_request("old-secret"), "kernel")
            .await
            .unwrap();

        let mut conflict = complete_request("replacement");
        conflict.feishu_login.app_secret.clear = true;
        assert!(service.update(conflict, "kernel").await.is_err());

        let mut clear = complete_request("");
        clear.feishu_login.enabled = false;
        clear.feishu_login.app_id = None;
        clear.feishu_login.redirect_uri = None;
        clear.feishu_login.app_secret.clear = true;
        let cleared = service.update(clear, "kernel").await.unwrap();
        assert!(!cleared.desired.feishu_login.app_secret_set);

        let mut short = complete_request("app-secret");
        short.external_options.token.value = Some("x".repeat(31));
        assert!(service.update(short, "kernel").await.is_err());
        let mut boundary = complete_request("app-secret");
        boundary.external_options.token.value = Some("x".repeat(32));
        assert!(service.update(boundary, "kernel").await.is_ok());
    }

    #[tokio::test]
    async fn approval_requires_login_and_applied_kernel_backend() {
        let (service, _) = setup().await;
        let mut without_login = approval_request();
        without_login.feishu_login.enabled = false;
        assert!(service.update(without_login, "kernel").await.is_err());
        let error = service
            .update(approval_request(), "userspace")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("wg_backend=kernel"));
        assert!(service.update(approval_request(), "kernel").await.is_ok());
    }

    #[tokio::test]
    async fn startup_rejects_disabled_approval_with_managed_users_and_blank_stored_values() {
        let (service, repo) = setup().await;
        sqlx::query(
            "INSERT INTO users (id,username,email,password_hash,role,status,must_change_password,created_at,updated_at,access_mode) VALUES ('managed','managed','managed@example.com','h','user','active',0,0,0,'approval_required')",
        )
        .execute(&service.pool)
        .await
        .unwrap();
        assert!(IntegrationSettingsService::load_or_seed(
            repo.clone(),
            service.pool.clone(),
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        )
        .await
        .is_err());

        sqlx::query("DELETE FROM users WHERE id='managed'")
            .execute(&service.pool)
            .await
            .unwrap();
        let mut blank = StoredIntegrationSettings::from_environment(
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        );
        blank.feishu_login.app_id = Some("   ".into());
        repo.set(SETTINGS_KEY, &serialize_settings(&blank).unwrap())
            .await
            .unwrap();
        assert!(IntegrationSettingsService::load_or_seed(
            repo,
            service.pool.clone(),
            &FeishuConfig::default(),
            &FeishuApprovalConfig::default(),
            &FeishuApprovalOptionsConfig::default(),
        )
        .await
        .is_err());
    }
}
