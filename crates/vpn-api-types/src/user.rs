//! 用户账号管理相关 DTO（Epic 3）。
//!
//! 这些类型是前后端共享契约：
//! - 后端 handler 反序列化请求 / 序列化响应
//! - 前端经 axios 拦截器以 camelCase 形式消费

use serde::{Deserialize, Serialize};

/// 对外暴露的用户视图（绝不包含 password_hash）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDto {
    pub id: String,
    pub username: String,
    pub email: String,
    /// "admin" | "user"
    pub role: String,
    /// "active" | "disabled"
    pub status: String,
    pub must_change_password: bool,
    /// 最后登录时间（unix ms），从未登录为 None
    pub last_login_at: Option<i64>,
    /// 所属用户组 id 列表（可属多个组；未分组为空，前端用组列表解析名称）。
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// 允许同时注册的终端数量上限（≥1，默认 1）。
    #[serde(default = "default_max_devices")]
    pub max_devices: i64,
    pub created_at: i64,
}

/// max_devices 的默认值（兼容旧服务端响应缺省该字段）。
fn default_max_devices() -> i64 {
    1
}

/// 创建用户请求。password 不传则后端自动生成 12 位强密码。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// 终端数量上限（不传默认 1）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_devices: Option<i64>,
}

/// 创建用户响应。initial_password 为明文，仅此一次返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserResponse {
    pub user: UserDto,
    pub initial_password: String,
}

/// 用户列表查询参数（GET /admin/users 的 query string）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListUsersQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    /// 模糊匹配 username / email
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// 按状态筛选："active" | "disabled"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 排序字段，默认 created_at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
}

/// 更新用户请求（PATCH）。支持改 status（禁用/启用）与 max_devices（终端上限），
/// 至少携带其一。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    /// "active" | "disabled"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 终端数量上限（≥1）。调小不影响已注册终端，仅限制后续新终端注册。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_devices: Option<i64>,
}

/// 重置密码响应。new_password 为明文，仅此一次返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetPasswordResponse {
    pub new_password: String,
}
