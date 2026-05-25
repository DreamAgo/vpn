//! 审计日志相关 DTO（Epic 5）。

use serde::{Deserialize, Serialize};

/// 单条审计日志（admin 后台展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogDto {
    pub id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    /// 动作类型，如 "user_create" / "peer_register" / "login_failed"
    pub action: String,
    /// 资源路径，如 "/api/v1/admin/users"
    pub resource: String,
    pub ip_addr: Option<String>,
    pub user_agent: Option<String>,
    /// 结构化补充信息（JSON 文本，前端按需 parse）
    pub metadata: Option<String>,
    pub status_code: Option<i32>,
    pub created_at: i64,
}

/// 审计日志查询参数（GET /admin/audit-logs）。
///
/// 不传任何时间筛选时，后端默认返回最近 7 天，避免全表扫描。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogQuery {
    /// 起始时间（unix ms，含）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// 结束时间（unix ms，含）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// 模糊匹配 username
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 精确匹配 action
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
}
