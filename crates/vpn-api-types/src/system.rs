//! 系统信息 DTO。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub vpn_subnet: String,
    pub server_public_key: String,
    pub server_endpoint: String,
    pub listen_port: u16,
    pub started_at: i64,
    /// 服务端配置的 LAN 网段（服务端作网关下发给客户端的 allowed_routes）。
    #[serde(default)]
    pub server_routes: Vec<String>,
}

/// 更新服务端 LAN 网段请求（PUT /api/v1/admin/system/routes）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateServerRoutesRequest {
    /// LAN 网段 CIDR 列表（空数组表示清空）。
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailNotificationSettings {
    pub enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password_set: bool,
    pub from: Option<String>,
    pub recipients: Vec<String>,
    #[serde(default = "default_quiet_minutes")]
    pub quiet_minutes: u32,
    #[serde(default = "default_true")]
    pub gateway_offline_enabled: bool,
    #[serde(default = "default_true")]
    pub gateway_recovered_enabled: bool,
    #[serde(default)]
    pub webhook: HttpNotificationChannelSettings,
    #[serde(default)]
    pub feishu: HttpNotificationChannelSettings,
    #[serde(default)]
    pub dingtalk: HttpNotificationChannelSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEmailNotificationSettingsRequest {
    pub enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    /// 留空或不传表示不修改当前密码；传空字符串表示清空密码。
    pub smtp_password: Option<String>,
    pub from: Option<String>,
    pub recipients: Vec<String>,
    #[serde(default = "default_quiet_minutes")]
    pub quiet_minutes: u32,
    #[serde(default = "default_true")]
    pub gateway_offline_enabled: bool,
    #[serde(default = "default_true")]
    pub gateway_recovered_enabled: bool,
    #[serde(default)]
    pub webhook: HttpNotificationChannelSettings,
    #[serde(default)]
    pub feishu: HttpNotificationChannelSettings,
    #[serde(default)]
    pub dingtalk: HttpNotificationChannelSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpNotificationChannelSettings {
    pub enabled: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestEmailNotificationRequest {
    pub recipient: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEventView {
    pub id: String,
    pub event_type: String,
    pub channel: String,
    pub target: String,
    pub status: String,
    pub subject: String,
    pub error: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub sent_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationEventQuery {
    pub event_type: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}

fn default_quiet_minutes() -> u32 {
    30
}

fn default_true() -> bool {
    true
}
