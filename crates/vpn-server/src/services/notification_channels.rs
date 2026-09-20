//! Pluggable notification channel implementations.

use async_trait::async_trait;
use lettre::{
    message::{Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use vpn_core::{AppError, Result};

use crate::config::NotificationConfig;

#[derive(Debug, Clone)]
pub struct NotificationMessage {
    pub event_type: String,
    pub target: String,
    pub subject: String,
    pub body: String,
    pub html_body: Option<String>,
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
        let email = build_email(&self.from, message)?;
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

fn build_email(from: &str, message: &NotificationMessage) -> Result<Message> {
    let builder = Message::builder()
        .from(parse_mailbox(from)?)
        .to(parse_mailbox(&message.target)?)
        .subject(&message.subject);
    if let Some(html) = &message.html_body {
        builder.multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(message.body.clone()))
                .singlepart(SinglePart::html(html.clone())),
        )
    } else {
        builder.singlepart(SinglePart::plain(message.body.clone()))
    }
    .map_err(|e| AppError::Internal(Box::new(e)))
}

#[cfg(test)]
mod html_mail_tests {
    use super::*;

    #[test]
    fn chinese_plain_mail_declares_mime_and_round_trips_utf8() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let message = NotificationMessage {
            event_type: "gateway_offline".into(),
            target: "user@example.com".into(),
            subject: "站点网关离线 - szjx-gateway".into(),
            body: "易链检测到站点网关离线。\n受影响网关：szjx-gateway\n请检查客户端网络、管理员权限、隧道进程和服务端连通性。".into(),
            html_body: None,
            metadata: None,
        };
        let mime = String::from_utf8(
            build_email("vpn@example.com", &message)
                .unwrap()
                .formatted(),
        )
        .unwrap();
        let (headers, encoded_body) = mime.split_once("\r\n\r\n").unwrap();
        assert!(headers.contains("MIME-Version: 1.0"));
        assert!(headers.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(headers.contains("Content-Transfer-Encoding: base64"));
        let decoded = STANDARD
            .decode(encoded_body.split_whitespace().collect::<String>())
            .unwrap();
        assert_eq!(
            String::from_utf8(decoded).unwrap(),
            message.body.replace('\n', "\r\n")
        );
    }

    #[test]
    fn html_mail_has_plain_alternative_and_legacy_mail_stays_plain() {
        let mut message = NotificationMessage {
            event_type: "approval_approved".into(),
            target: "user@example.com".into(),
            subject: "Approved".into(),
            body: "Approved account".into(),
            html_body: Some("<h1>Approved account</h1>".into()),
            metadata: None,
        };
        let mime = String::from_utf8(
            build_email("vpn@example.com", &message)
                .unwrap()
                .formatted(),
        )
        .unwrap();
        assert!(mime.contains("multipart/alternative"));
        assert!(mime.contains("text/plain"));
        assert!(mime.contains("text/html"));
        assert!(mime.find("text/plain").unwrap() < mime.find("text/html").unwrap());
        message.html_body = None;
        let mime = String::from_utf8(
            build_email("vpn@example.com", &message)
                .unwrap()
                .formatted(),
        )
        .unwrap();
        assert!(!mime.contains("text/html"));
        assert!(mime.contains("Approved account"));
    }
}
