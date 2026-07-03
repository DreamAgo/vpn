//! Pluggable notification channel implementations.

use async_trait::async_trait;
use lettre::{
    message::Mailbox, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};
use vpn_core::{AppError, Result};

use crate::config::NotificationConfig;

#[derive(Debug, Clone)]
pub struct NotificationMessage {
    pub event_type: String,
    pub target: String,
    pub subject: String,
    pub body: String,
    pub metadata: Option<String>,
}

#[async_trait]
pub trait Notifier: Send + Sync {
    fn channel(&self) -> &'static str;
    async fn send(&self, message: &NotificationMessage) -> Result<()>;
}

pub struct EmailNotifier {
    from: String,
    mailer: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailNotifier {
    pub fn from_config(config: &NotificationConfig) -> Option<Self> {
        if !config.email_enabled {
            return None;
        }
        let host = config.smtp_host.as_deref()?;
        let from = config.email_from.clone()?;
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .ok()?
            .port(config.smtp_port);
        if let (Some(username), Some(password)) = (&config.smtp_username, &config.smtp_password) {
            builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
        }
        Some(Self {
            from,
            mailer: builder.build(),
        })
    }
}

#[async_trait]
impl Notifier for EmailNotifier {
    fn channel(&self) -> &'static str {
        "email"
    }

    async fn send(&self, message: &NotificationMessage) -> Result<()> {
        let email = Message::builder()
            .from(parse_mailbox(&self.from)?)
            .to(parse_mailbox(&message.target)?)
            .subject(&message.subject)
            .body(message.body.clone())
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        self.mailer
            .send(email)
            .await
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        Ok(())
    }
}

pub struct HttpNotifier {
    channel: &'static str,
    url: String,
}

impl HttpNotifier {
    pub fn new(channel: &'static str, url: &str) -> Self {
        Self {
            channel,
            url: url.to_string(),
        }
    }
}

#[async_trait]
impl Notifier for HttpNotifier {
    fn channel(&self) -> &'static str {
        self.channel
    }

    async fn send(&self, message: &NotificationMessage) -> Result<()> {
        let payload = http_payload(
            self.channel,
            &message.event_type,
            &message.subject,
            &message.body,
            message.metadata.as_deref(),
        );
        let resp = reqwest::Client::new()
            .post(&self.url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        if !resp.status().is_success() {
            return Err(AppError::Validation(format!(
                "{} 通知发送失败：HTTP {}",
                self.channel,
                resp.status()
            )));
        }
        Ok(())
    }
}

pub fn parse_mailbox(raw: &str) -> Result<Mailbox> {
    raw.parse()
        .map_err(|e| AppError::Validation(format!("邮件地址无效 {raw}: {e}")))
}

fn http_payload(
    channel: &str,
    event_type: &str,
    subject: &str,
    body: &str,
    metadata: Option<&str>,
) -> serde_json::Value {
    match channel {
        "feishu" => serde_json::json!({
            "msg_type": "text",
            "content": { "text": format!("{subject}\n\n{body}") }
        }),
        "dingtalk" => serde_json::json!({
            "msgtype": "text",
            "text": { "content": format!("{subject}\n\n{body}") }
        }),
        _ => serde_json::json!({
            "event_type": event_type,
            "title": subject,
            "text": body,
            "metadata": metadata.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        }),
    }
}
