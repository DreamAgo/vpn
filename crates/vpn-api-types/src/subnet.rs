//! 网段目录 DTO。集中维护命名网段(名称+CIDR),供各处下拉选择。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetDto {
    pub id: String,
    pub name: String,
    pub cidr: String,
    /// 该 CIDR 当前被引用的次数(用户组路由 + 节点路由 + 服务端 LAN 之和)。
    #[serde(default)]
    pub usage_count: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubnetRequest {
    pub name: String,
    pub cidr: String,
}

/// 更新网段(字段缺省表示不改)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSubnetRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
}
