//! 用户组管理相关 DTO。
//!
//! 用户组持有一组"可路由网段"(CIDR);成员的 VPN allowed_routes 由该组决定
//! (访问控制)。前后端共享契约,经 axios 拦截器 camelCase ↔ snake_case 互转。

use serde::{Deserialize, Serialize};

/// 对外暴露的用户组视图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroupDto {
    pub id: String,
    pub name: String,
    /// 该组可路由的网段(归一化后的 CIDR 列表)。
    pub routes: Vec<String>,
    /// 当前归属该组的用户数。
    pub member_count: u32,
    pub created_at: i64,
}

/// 创建用户组请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserGroupRequest {
    pub name: String,
    /// 可路由网段(CIDR);允许为空(成员只放行 VPN 子网 + 站点网关)。
    #[serde(default)]
    pub routes: Vec<String>,
}

/// 更新用户组请求(PATCH;字段缺省表示不改)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserGroupRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<String>>,
}

/// 设置某用户所属组(全量覆盖)的请求。空列表表示取消所有分组。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignGroupRequest {
    #[serde(default)]
    pub group_ids: Vec<String>,
}
