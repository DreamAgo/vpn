//! 认证相关 DTO。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_in: i64,
    pub must_change_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub access_expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstTimeSetupRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstTimeSetupResponse {
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusResponse {
    pub needs_setup: bool,
}

/// 飞书登录能力探测。未配置时客户端仍可使用密码登录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuAuthConfigResponse {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuAuthStartResponse {
    pub authorization_url: String,
    pub poll_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuAuthPollRequest {
    pub poll_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeishuAuthPollStatus {
    Pending,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuAuthPollResponse {
    pub status: FeishuAuthPollStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginResponse>,
}
