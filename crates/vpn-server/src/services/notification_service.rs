//! Event notification service.

use reqwest::Url;
use uuid::Uuid;
use vpn_api_types::system::{
    EmailNotificationSettings, HttpNotificationChannelSettings, NotificationEventQuery,
    NotificationEventView, TestEmailNotificationRequest, UpdateEmailNotificationSettingsRequest,
};
use vpn_core::{AppError, Result};

use crate::{
    config::NotificationConfig,
    repositories::SqliteNotificationEventRepository,
    services::{
        config_service::ConfigService,
        notification_channels::{
            parse_mailbox, EmailNotifier, HttpNotifier, NotificationMessage, Notifier,
        },
        GatewayOfflineNotice,
    },
};

const KEY_NOTIFY_EMAIL_ENABLED: &str = "notify_email_enabled";
const KEY_NOTIFY_SMTP_HOST: &str = "notify_smtp_host";
const KEY_NOTIFY_SMTP_PORT: &str = "notify_smtp_port";
const KEY_NOTIFY_SMTP_USERNAME: &str = "notify_smtp_username";
const KEY_NOTIFY_SMTP_PASSWORD: &str = "notify_smtp_password";
const KEY_NOTIFY_EMAIL_FROM: &str = "notify_email_from";
const KEY_NOTIFY_EMAIL_TO: &str = "notify_email_to";
const KEY_NOTIFY_QUIET_MINUTES: &str = "notify_quiet_minutes";
const KEY_NOTIFY_GATEWAY_OFFLINE: &str = "notify_gateway_offline";
const KEY_NOTIFY_GATEWAY_RECOVERED: &str = "notify_gateway_recovered";
const KEY_NOTIFY_WEBHOOK_ENABLED: &str = "notify_webhook_enabled";
const KEY_NOTIFY_WEBHOOK_URL: &str = "notify_webhook_url";
const KEY_NOTIFY_FEISHU_ENABLED: &str = "notify_feishu_enabled";
const KEY_NOTIFY_FEISHU_URL: &str = "notify_feishu_url";
const KEY_NOTIFY_DINGTALK_ENABLED: &str = "notify_dingtalk_enabled";
const KEY_NOTIFY_DINGTALK_URL: &str = "notify_dingtalk_url";

const EVENT_GATEWAY_OFFLINE: &str = "gateway_offline";
const EVENT_GATEWAY_RECOVERED: &str = "gateway_recovered";
const EVENT_TEST_EMAIL: &str = "test_email";
const CHANNEL_EMAIL: &str = "email";
const CHANNEL_WEBHOOK: &str = "webhook";
const CHANNEL_FEISHU: &str = "feishu";
const CHANNEL_DINGTALK: &str = "dingtalk";

#[derive(Debug, Clone, Copy)]
struct NotificationRules {
    quiet_minutes: u32,
    gateway_offline_enabled: bool,
    gateway_recovered_enabled: bool,
}

#[derive(Debug, Clone, Default)]
struct HttpChannels {
    webhook: HttpNotificationChannelSettings,
    feishu: HttpNotificationChannelSettings,
    dingtalk: HttpNotificationChannelSettings,
}

#[derive(Clone)]
pub struct NotificationService {
    defaults: NotificationConfig,
    config_service: Option<ConfigService>,
    event_repo: Option<SqliteNotificationEventRepository>,
}

impl NotificationService {
    pub fn new(config: NotificationConfig) -> Self {
        if config.email_enabled && EmailNotifier::from_config(&config).is_none() {
            tracing::warn!("邮件通知已启用，但 SMTP 配置不完整，通知将不会发送");
        }
        Self {
            defaults: config,
            config_service: None,
            event_repo: None,
        }
    }

    pub fn new_with_config_service(
        config: NotificationConfig,
        config_service: ConfigService,
        event_repo: SqliteNotificationEventRepository,
    ) -> Self {
        let mut service = Self::new(config);
        service.config_service = Some(config_service);
        service.event_repo = Some(event_repo);
        service
    }

    pub async fn email_settings(&self) -> Result<EmailNotificationSettings> {
        let config = self.effective_config().await?;
        let rules = self.rules().await?;
        let channels = self.http_channels().await?;
        Ok(config_to_settings(&config, rules, channels))
    }

    pub async fn update_email_settings(
        &self,
        req: UpdateEmailNotificationSettingsRequest,
    ) -> Result<EmailNotificationSettings> {
        let config_store = self
            .config_service
            .as_ref()
            .ok_or_else(|| AppError::Config("config_service 未初始化".to_string()))?;
        let current = self.effective_config().await?;
        let smtp_host = clean_opt(req.smtp_host);
        let smtp_username = clean_opt(req.smtp_username);
        let from = clean_opt(req.from);
        let recipients = req
            .recipients
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let quiet_minutes = req.quiet_minutes.clamp(0, 1440);
        validate_http_channel(CHANNEL_WEBHOOK, &req.webhook)?;
        validate_http_channel(CHANNEL_FEISHU, &req.feishu)?;
        validate_http_channel(CHANNEL_DINGTALK, &req.dingtalk)?;

        if req.enabled {
            let has_email = smtp_host.is_some() && from.is_some() && !recipients.is_empty();
            let has_http = req.webhook.enabled || req.feishu.enabled || req.dingtalk.enabled;
            if !has_email && !has_http {
                return Err(AppError::Validation(
                    "启用通知后至少配置一个可用渠道".to_string(),
                ));
            }
            if has_email {
                parse_mailbox(from.as_deref().unwrap())?;
                for recipient in &recipients {
                    parse_mailbox(recipient)?;
                }
            }
        }

        config_store
            .set_bool(KEY_NOTIFY_EMAIL_ENABLED, req.enabled)
            .await?;
        config_store
            .set_string(KEY_NOTIFY_SMTP_HOST, smtp_host.as_deref())
            .await?;
        config_store
            .set_u16(KEY_NOTIFY_SMTP_PORT, req.smtp_port)
            .await?;
        config_store
            .set_string(KEY_NOTIFY_SMTP_USERNAME, smtp_username.as_deref())
            .await?;
        if let Some(password) = req.smtp_password {
            config_store
                .set_string(KEY_NOTIFY_SMTP_PASSWORD, Some(password.trim()))
                .await?;
        } else if current.smtp_password.is_none() {
            config_store
                .set_string(KEY_NOTIFY_SMTP_PASSWORD, None)
                .await?;
        }
        config_store
            .set_string(KEY_NOTIFY_EMAIL_FROM, from.as_deref())
            .await?;
        config_store
            .set_csv(KEY_NOTIFY_EMAIL_TO, &recipients)
            .await?;
        config_store
            .set_u32(KEY_NOTIFY_QUIET_MINUTES, quiet_minutes)
            .await?;
        config_store
            .set_bool(KEY_NOTIFY_GATEWAY_OFFLINE, req.gateway_offline_enabled)
            .await?;
        config_store
            .set_bool(KEY_NOTIFY_GATEWAY_RECOVERED, req.gateway_recovered_enabled)
            .await?;
        self.save_http_channel(
            config_store,
            KEY_NOTIFY_WEBHOOK_ENABLED,
            KEY_NOTIFY_WEBHOOK_URL,
            &req.webhook,
        )
        .await?;
        self.save_http_channel(
            config_store,
            KEY_NOTIFY_FEISHU_ENABLED,
            KEY_NOTIFY_FEISHU_URL,
            &req.feishu,
        )
        .await?;
        self.save_http_channel(
            config_store,
            KEY_NOTIFY_DINGTALK_ENABLED,
            KEY_NOTIFY_DINGTALK_URL,
            &req.dingtalk,
        )
        .await?;

        let updated = self.effective_config().await?;
        let rules = self.rules().await?;
        let channels = self.http_channels().await?;
        Ok(config_to_settings(&updated, rules, channels))
    }

    pub async fn notify_gateway_offline(&self, gateways: &[GatewayOfflineNotice]) -> Result<()> {
        let rules = self.rules().await?;
        if !rules.gateway_offline_enabled {
            return Ok(());
        }
        self.notify_gateways(EVENT_GATEWAY_OFFLINE, gateways).await
    }

    pub async fn notify_gateway_recovered(&self, gateways: &[GatewayOfflineNotice]) -> Result<()> {
        let rules = self.rules().await?;
        if !rules.gateway_recovered_enabled {
            return Ok(());
        }
        self.notify_gateways(EVENT_GATEWAY_RECOVERED, gateways)
            .await
    }

    pub async fn send_test_email(&self, req: TestEmailNotificationRequest) -> Result<()> {
        let config = self.effective_config().await?;
        let recipient = req
            .recipient
            .and_then(clean_string)
            .or_else(|| config.email_to.first().cloned())
            .unwrap_or_else(|| "*".to_string());
        let subject = "易链测试通知".to_string();
        let body = "这是一封易链事件通知测试邮件。收到此邮件表示 SMTP 配置可用。".to_string();
        let metadata = serde_json::json!({"test": true}).to_string();
        let mut sent_any = false;
        if recipient != "*" {
            parse_mailbox(&recipient)?;
            self.send_email_and_record(
                EVENT_TEST_EMAIL,
                &recipient,
                &subject,
                &body,
                "test_email",
                Some(&metadata),
                false,
            )
            .await?;
            sent_any = true;
        }
        let channels = self.http_channels().await?;
        for (channel, config) in enabled_http_channels(&channels) {
            self.send_http_and_record(
                EVENT_TEST_EMAIL,
                channel,
                config.url.as_deref().unwrap_or_default(),
                &subject,
                &body,
                "test_email",
                Some(&metadata),
            )
            .await?;
            sent_any = true;
        }
        if !sent_any {
            return Err(AppError::Validation("请先配置至少一个通知渠道".to_string()));
        }
        Ok(())
    }

    pub async fn list_events(
        &self,
        query: &NotificationEventQuery,
    ) -> Result<Vec<NotificationEventView>> {
        let repo = self.event_repo()?;
        let rows = repo
            .list(
                clean_opt(query.event_type.clone()).as_deref(),
                clean_opt(query.status.clone()).as_deref(),
                query.limit.unwrap_or(50).clamp(1, 200),
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| NotificationEventView {
                id: r.id,
                event_type: r.event_type,
                channel: r.channel,
                target: r.target,
                status: r.status,
                subject: r.subject,
                error: r.error,
                metadata: r.metadata,
                created_at: r.created_at,
                sent_at: r.sent_at,
            })
            .collect())
    }

    async fn notify_gateways(
        &self,
        event_type: &str,
        gateways: &[GatewayOfflineNotice],
    ) -> Result<()> {
        if gateways.is_empty() {
            return Ok(());
        }
        let config = self.effective_config().await?;
        let channels = self.http_channels().await?;
        let email_ready =
            config.email_enabled && !config.email_to.is_empty() && config.email_from.is_some();
        let http_channels = enabled_http_channels(&channels);
        if !email_ready && http_channels.is_empty() {
            return Ok(());
        }

        let rules = self.rules().await?;
        for gateway in gateways {
            let dedupe_key = format!("{event_type}:{}", gateway.peer_id);
            if self
                .is_quiet_period(&dedupe_key, rules.quiet_minutes)
                .await?
            {
                self.record_event(
                    event_type,
                    CHANNEL_EMAIL,
                    "*",
                    "skipped",
                    &subject_for(event_type, std::slice::from_ref(gateway)),
                    Some("静默期内已通知，跳过重复发送"),
                    Some(&gateway_metadata(gateway)),
                    &dedupe_key,
                    None,
                )
                .await?;
                continue;
            }
            let one = std::slice::from_ref(gateway);
            let subject = subject_for(event_type, one);
            let body = render_gateway_body(event_type, one);
            let metadata = gateway_metadata(gateway);
            if email_ready {
                for to in &config.email_to {
                    self.send_email_and_record(
                        event_type,
                        to,
                        &subject,
                        &body,
                        &dedupe_key,
                        Some(&metadata),
                        true,
                    )
                    .await?;
                }
            }
            for (channel, http_config) in &http_channels {
                self.send_http_and_record(
                    event_type,
                    channel,
                    http_config.url.as_deref().unwrap_or_default(),
                    &subject,
                    &body,
                    &dedupe_key,
                    Some(&metadata),
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn effective_config(&self) -> Result<NotificationConfig> {
        let Some(config_store) = &self.config_service else {
            return Ok(self.defaults.clone());
        };
        let enabled = config_store
            .get_bool(KEY_NOTIFY_EMAIL_ENABLED, self.defaults.email_enabled)
            .await?;
        let smtp_host = config_store
            .get_string(KEY_NOTIFY_SMTP_HOST)
            .await?
            .or_else(|| self.defaults.smtp_host.clone());
        let smtp_port = config_store
            .get_u16(KEY_NOTIFY_SMTP_PORT, self.defaults.smtp_port)
            .await?;
        let smtp_username = config_store
            .get_string(KEY_NOTIFY_SMTP_USERNAME)
            .await?
            .or_else(|| self.defaults.smtp_username.clone());
        let smtp_password = config_store
            .get_string(KEY_NOTIFY_SMTP_PASSWORD)
            .await?
            .or_else(|| self.defaults.smtp_password.clone());
        let email_from = config_store
            .get_string(KEY_NOTIFY_EMAIL_FROM)
            .await?
            .or_else(|| self.defaults.email_from.clone());
        let email_to = config_store
            .get_csv(KEY_NOTIFY_EMAIL_TO, &self.defaults.email_to)
            .await?;
        Ok(NotificationConfig {
            email_enabled: enabled,
            smtp_host,
            smtp_port,
            smtp_username,
            smtp_password,
            email_from,
            email_to,
        })
    }

    async fn rules(&self) -> Result<NotificationRules> {
        let Some(config_store) = &self.config_service else {
            return Ok(NotificationRules {
                quiet_minutes: 30,
                gateway_offline_enabled: true,
                gateway_recovered_enabled: true,
            });
        };
        let quiet_minutes = config_store
            .get_u32(KEY_NOTIFY_QUIET_MINUTES, 30)
            .await?
            .min(1440);
        let gateway_offline_enabled = config_store
            .get_bool(KEY_NOTIFY_GATEWAY_OFFLINE, true)
            .await?;
        let gateway_recovered_enabled = config_store
            .get_bool(KEY_NOTIFY_GATEWAY_RECOVERED, true)
            .await?;
        Ok(NotificationRules {
            quiet_minutes,
            gateway_offline_enabled,
            gateway_recovered_enabled,
        })
    }

    async fn http_channels(&self) -> Result<HttpChannels> {
        let Some(config_store) = &self.config_service else {
            return Ok(HttpChannels::default());
        };
        Ok(HttpChannels {
            webhook: self
                .load_http_channel(
                    config_store,
                    KEY_NOTIFY_WEBHOOK_ENABLED,
                    KEY_NOTIFY_WEBHOOK_URL,
                )
                .await?,
            feishu: self
                .load_http_channel(
                    config_store,
                    KEY_NOTIFY_FEISHU_ENABLED,
                    KEY_NOTIFY_FEISHU_URL,
                )
                .await?,
            dingtalk: self
                .load_http_channel(
                    config_store,
                    KEY_NOTIFY_DINGTALK_ENABLED,
                    KEY_NOTIFY_DINGTALK_URL,
                )
                .await?,
        })
    }

    async fn load_http_channel(
        &self,
        config_store: &ConfigService,
        enabled_key: &str,
        url_key: &str,
    ) -> Result<HttpNotificationChannelSettings> {
        let enabled = config_store.get_bool(enabled_key, false).await?;
        let url = config_store.get_string(url_key).await?;
        Ok(HttpNotificationChannelSettings { enabled, url })
    }

    async fn save_http_channel(
        &self,
        config_store: &ConfigService,
        enabled_key: &str,
        url_key: &str,
        channel: &HttpNotificationChannelSettings,
    ) -> Result<()> {
        config_store.set_bool(enabled_key, channel.enabled).await?;
        config_store
            .set_string(url_key, channel.url.as_deref())
            .await
    }

    fn event_repo(&self) -> Result<&SqliteNotificationEventRepository> {
        self.event_repo
            .as_ref()
            .ok_or_else(|| AppError::Config("notification_event_repo 未初始化".to_string()))
    }

    async fn is_quiet_period(&self, dedupe_key: &str, quiet_minutes: u32) -> Result<bool> {
        if quiet_minutes == 0 {
            return Ok(false);
        }
        let after = chrono::Utc::now().timestamp_millis() - i64::from(quiet_minutes) * 60_000;
        Ok(self
            .event_repo()?
            .latest_after(dedupe_key, after)
            .await?
            .is_some())
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_email_and_record(
        &self,
        event_type: &str,
        target: &str,
        subject: &str,
        body: &str,
        dedupe_key: &str,
        metadata: Option<&str>,
        require_enabled: bool,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let config = self.effective_config().await?;
        let can_send = (!require_enabled || config.email_enabled)
            && EmailNotifier::from_config(&config).is_some();
        if !can_send {
            self.record_event(
                event_type,
                CHANNEL_EMAIL,
                target,
                "failed",
                subject,
                Some("邮件通知未启用或 SMTP 配置不完整"),
                metadata,
                dedupe_key,
                None,
            )
            .await?;
            return Err(AppError::Validation(
                "邮件通知未启用或 SMTP 配置不完整".to_string(),
            ));
        }
        let notifier = EmailNotifier::from_config(&config)
            .ok_or_else(|| AppError::Validation("SMTP 配置不完整".to_string()))?;
        let result = notifier
            .send(&NotificationMessage {
                event_type: event_type.to_string(),
                target: target.to_string(),
                subject: subject.to_string(),
                body: body.to_string(),
                metadata: metadata.map(str::to_string),
            })
            .await;

        match result {
            Ok(()) => {
                self.record_event(
                    event_type,
                    CHANNEL_EMAIL,
                    target,
                    "sent",
                    subject,
                    None,
                    metadata,
                    dedupe_key,
                    Some(now),
                )
                .await?;
                Ok(())
            }
            Err(e) => {
                self.record_event(
                    event_type,
                    CHANNEL_EMAIL,
                    target,
                    "failed",
                    subject,
                    Some(&e.to_string()),
                    metadata,
                    dedupe_key,
                    None,
                )
                .await?;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_http_and_record(
        &self,
        event_type: &str,
        channel: &str,
        url: &str,
        subject: &str,
        body: &str,
        dedupe_key: &str,
        metadata: Option<&str>,
    ) -> Result<()> {
        let notifier = HttpNotifier::new(
            match channel {
                CHANNEL_FEISHU => CHANNEL_FEISHU,
                CHANNEL_DINGTALK => CHANNEL_DINGTALK,
                CHANNEL_WEBHOOK => CHANNEL_WEBHOOK,
                _ => CHANNEL_WEBHOOK,
            },
            url,
        );
        let result = notifier
            .send(&NotificationMessage {
                event_type: event_type.to_string(),
                target: url.to_string(),
                subject: subject.to_string(),
                body: body.to_string(),
                metadata: metadata.map(str::to_string),
            })
            .await;
        match result {
            Ok(()) => {
                self.record_event(
                    event_type,
                    channel,
                    url,
                    "sent",
                    subject,
                    None,
                    metadata,
                    dedupe_key,
                    Some(chrono::Utc::now().timestamp_millis()),
                )
                .await?;
                Ok(())
            }
            Err(e) => {
                self.record_event(
                    event_type,
                    channel,
                    url,
                    "failed",
                    subject,
                    Some(&e.to_string()),
                    metadata,
                    dedupe_key,
                    None,
                )
                .await?;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_event(
        &self,
        event_type: &str,
        channel: &str,
        target: &str,
        status: &str,
        subject: &str,
        error: Option<&str>,
        metadata: Option<&str>,
        dedupe_key: &str,
        sent_at: Option<i64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        self.event_repo()?
            .insert(
                &Uuid::now_v7().to_string(),
                event_type,
                channel,
                target,
                status,
                subject,
                error,
                metadata,
                dedupe_key,
                sent_at,
                now,
            )
            .await
    }
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(clean_string)
}

fn clean_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn config_to_settings(
    config: &NotificationConfig,
    rules: NotificationRules,
    channels: HttpChannels,
) -> EmailNotificationSettings {
    EmailNotificationSettings {
        enabled: config.email_enabled,
        smtp_host: config.smtp_host.clone(),
        smtp_port: config.smtp_port,
        smtp_username: config.smtp_username.clone(),
        smtp_password_set: config
            .smtp_password
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        from: config.email_from.clone(),
        recipients: config.email_to.clone(),
        quiet_minutes: rules.quiet_minutes,
        gateway_offline_enabled: rules.gateway_offline_enabled,
        gateway_recovered_enabled: rules.gateway_recovered_enabled,
        webhook: channels.webhook,
        feishu: channels.feishu,
        dingtalk: channels.dingtalk,
    }
}

fn validate_http_channel(name: &str, channel: &HttpNotificationChannelSettings) -> Result<()> {
    if !channel.enabled {
        return Ok(());
    }
    let url = channel
        .url
        .as_deref()
        .ok_or_else(|| AppError::Validation(format!("{name} URL 不能为空")))?;
    let parsed = Url::parse(url).map_err(|_| AppError::Validation(format!("{name} URL 无效")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::Validation(format!(
            "{name} URL 只支持 http/https"
        )));
    }
    Ok(())
}

fn enabled_http_channels(
    channels: &HttpChannels,
) -> Vec<(&'static str, &HttpNotificationChannelSettings)> {
    let mut out = Vec::new();
    if channels.webhook.enabled && channels.webhook.url.is_some() {
        out.push((CHANNEL_WEBHOOK, &channels.webhook));
    }
    if channels.feishu.enabled && channels.feishu.url.is_some() {
        out.push((CHANNEL_FEISHU, &channels.feishu));
    }
    if channels.dingtalk.enabled && channels.dingtalk.url.is_some() {
        out.push((CHANNEL_DINGTALK, &channels.dingtalk));
    }
    out
}

fn subject_for(event_type: &str, gateways: &[GatewayOfflineNotice]) -> String {
    let event_name = match event_type {
        EVENT_GATEWAY_RECOVERED => "站点网关恢复",
        _ => "站点网关离线",
    };
    if gateways.len() == 1 {
        format!("易链通知：{event_name} - {}", gateways[0].device_name)
    } else {
        format!("易链通知：{} 个{event_name}", gateways.len())
    }
}

fn render_gateway_body(event_type: &str, gateways: &[GatewayOfflineNotice]) -> String {
    let intro = match event_type {
        EVENT_GATEWAY_RECOVERED => "易链检测到站点网关恢复在线。",
        _ => "易链检测到站点网关离线。",
    };
    let mut lines = vec![intro.to_string(), String::new(), "受影响网关：".to_string()];
    for gateway in gateways {
        lines.push(format!("- 设备：{}", gateway.device_name));
        lines.push(format!("  节点 ID：{}", gateway.peer_id));
        lines.push(format!("  用户 ID：{}", gateway.user_id));
        lines.push(format!("  虚拟 IP：{}", gateway.vpn_ip));
        lines.push(format!("  承载网段：{}", gateway.routed_subnets.join(", ")));
        if let Some(last_seen_at) = gateway.last_seen_at {
            lines.push(format!("  最后心跳：{}", last_seen_at));
        }
        lines.push(String::new());
    }
    if event_type == EVENT_GATEWAY_RECOVERED {
        lines.push("请确认业务侧访问已经恢复。".to_string());
    } else {
        lines.push("请检查客户端网络、管理员权限、隧道进程和服务端连通性。".to_string());
    }
    lines.join("\n")
}

fn gateway_metadata(gateway: &GatewayOfflineNotice) -> String {
    serde_json::json!({
        "peer_id": gateway.peer_id,
        "user_id": gateway.user_id,
        "device_name": gateway.device_name,
        "vpn_ip": gateway.vpn_ip,
        "routed_subnets": gateway.routed_subnets,
        "last_seen_at": gateway.last_seen_at,
    })
    .to_string()
}
