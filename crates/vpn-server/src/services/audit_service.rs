//! 审计日志业务服务（Epic 5：写入 / 查询 / 清理）。
//!
//! 写入是「尽力而为」：失败仅降级为 tracing::warn，不阻塞主请求路径。

use uuid::Uuid;
use vpn_api_types::{
    audit::{AuditLogDto, AuditLogQuery},
    Page,
};
use vpn_core::Result;

use crate::repositories::{
    audit_log_repo_sqlite::AuditLogRow, AuditLogEntry, AuditLogFilter, SqliteAuditLogRepository,
};

/// 审计查询默认时间窗：不传 from/to 时返回最近 7 天。
pub const DEFAULT_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const DEFAULT_PAGE_SIZE: u32 = 20;
const MAX_PAGE_SIZE: u32 = 100;

#[derive(Clone)]
pub struct AuditService {
    repo: SqliteAuditLogRepository,
}

impl AuditService {
    pub fn new(repo: SqliteAuditLogRepository) -> Self {
        Self { repo }
    }

    /// 写入一条审计日志。失败不返回错误，仅 warn（审计不应阻塞业务）。
    pub async fn log(&self, entry: AuditLogEntry, now_ms: i64) {
        let id = Uuid::now_v7().to_string();
        if let Err(e) = self.repo.insert(&id, &entry, now_ms).await {
            tracing::warn!(error = ?e, action = %entry.action, "审计日志写入失败（已降级，不影响主流程）");
        }
    }

    /// 记录登录尝试（成功/失败）。username 总是已知；失败时 user_id 为 None。
    pub async fn log_login_attempt(
        &self,
        username: &str,
        success: bool,
        reason: Option<&str>,
        ip: Option<&str>,
        now_ms: i64,
    ) {
        let action = if success {
            "login_success"
        } else {
            "login_failed"
        };
        let metadata = reason.map(|r| format!(r#"{{"reason":"{}"}}"#, r.replace('"', "'")));
        let entry = AuditLogEntry {
            user_id: None,
            username: Some(username.to_string()),
            action: action.to_string(),
            resource: "/api/v1/auth/login".to_string(),
            ip_addr: ip.map(|s| s.to_string()),
            user_agent: None,
            metadata,
            status_code: None,
        };
        self.log(entry, now_ms).await;
    }

    /// 删除早于 cutoff_ms 的日志。返回删除条数。
    pub async fn purge_older_than(&self, cutoff_ms: i64) -> Result<u64> {
        self.repo.delete_older_than(cutoff_ms).await
    }

    /// Story 5.4：分页查询审计日志。
    ///
    /// 不传 from/to 时默认返回最近 7 天（now - 7d ..= now）。
    pub async fn query(&self, query: &AuditLogQuery, now_ms: i64) -> Result<Page<AuditLogDto>> {
        let filter = build_filter(query, now_ms);
        let total = self.repo.count(&filter).await? as u64;
        let rows = self.repo.list(&filter).await?;
        let items = rows.into_iter().map(row_to_dto).collect();
        Ok(Page::new(items, total, filter.page, filter.page_size))
    }
}

/// 把查询参数归一化为仓储过滤条件（套用默认时间窗 + 分页默认值）。
fn build_filter(query: &AuditLogQuery, now_ms: i64) -> AuditLogFilter {
    // from/to 缺省策略：
    // - 都不传 → 最近 7 天 [now-7d, now]
    // - 只传 from → [from, now]
    // - 只传 to → [to-7d, to]
    // - 都传 → [from, to]
    let (from, to) = match (query.from, query.to) {
        (Some(f), Some(t)) => (f, t),
        (Some(f), None) => (f, now_ms),
        (None, Some(t)) => (t - DEFAULT_WINDOW_MS, t),
        (None, None) => (now_ms - DEFAULT_WINDOW_MS, now_ms),
    };
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    AuditLogFilter {
        from,
        to,
        user_id: query.user_id.clone(),
        username: query.username.clone(),
        action: query.action.clone(),
        page,
        page_size,
    }
}

fn row_to_dto(r: AuditLogRow) -> AuditLogDto {
    AuditLogDto {
        id: r.id,
        user_id: r.user_id,
        username: r.username,
        action: r.action,
        resource: r.resource,
        ip_addr: r.ip_addr,
        user_agent: r.user_agent,
        metadata: r.metadata,
        status_code: r.status_code,
        created_at: r.created_at,
    }
}

/// 由 HTTP method + path 推断审计 action（中间件用）。
///
/// 已知端点映射到语义动作；未知写操作回退为 `{method_lower}_request`。
pub fn infer_action(method: &str, path: &str) -> String {
    let m = method.to_ascii_uppercase();
    // 归一化路径：去掉末尾斜杠，便于匹配。
    let p = path.trim_end_matches('/');
    match (m.as_str(), p) {
        ("POST", "/api/v1/admin/users") => "user_create".to_string(),
        ("POST", "/api/v1/auth/first-time-setup") => "first_time_setup".to_string(),
        ("POST", "/api/v1/auth/login") => "login".to_string(),
        ("POST", "/api/v1/auth/logout") => "logout".to_string(),
        ("POST", "/api/v1/auth/change-password") => "change_password".to_string(),
        ("POST", "/api/v1/peers/register") => "peer_register".to_string(),
        ("POST", "/api/v1/peers/heartbeat") => "peer_heartbeat".to_string(),
        ("DELETE", "/api/v1/peers/me") => "peer_delete".to_string(),
        _ => {
            // 带路径参数的端点用前缀 + 后缀匹配。
            if m == "PATCH" && is_admin_user_id(p) {
                return "user_update".to_string();
            }
            if m == "DELETE" && is_admin_user_id(p) {
                return "user_delete".to_string();
            }
            if m == "POST"
                && p.starts_with("/api/v1/admin/users/")
                && p.ends_with("/reset-password")
            {
                return "user_reset_password".to_string();
            }
            if m == "DELETE" && is_admin_peer_id(p) {
                return "peer_force_remove".to_string();
            }
            format!("{}_request", m.to_ascii_lowercase())
        }
    }
}

/// 是否匹配 `/api/v1/admin/users/{id}`（无更深层级）。
fn is_admin_user_id(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("/api/v1/admin/users/") {
        !rest.is_empty() && !rest.contains('/')
    } else {
        false
    }
}

/// 是否匹配 `/api/v1/admin/peers/{id}`（无更深层级）。
fn is_admin_peer_id(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("/api/v1/admin/peers/") {
        !rest.is_empty() && !rest.contains('/')
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_action_known_endpoints() {
        assert_eq!(infer_action("POST", "/api/v1/admin/users"), "user_create");
        assert_eq!(
            infer_action("PATCH", "/api/v1/admin/users/abc-123"),
            "user_update"
        );
        assert_eq!(
            infer_action("DELETE", "/api/v1/admin/users/abc-123"),
            "user_delete"
        );
        assert_eq!(
            infer_action("POST", "/api/v1/admin/users/abc/reset-password"),
            "user_reset_password"
        );
        assert_eq!(
            infer_action("POST", "/api/v1/peers/register"),
            "peer_register"
        );
        assert_eq!(infer_action("DELETE", "/api/v1/peers/me"), "peer_delete");
        assert_eq!(
            infer_action("DELETE", "/api/v1/admin/peers/p-1"),
            "peer_force_remove"
        );
    }

    #[test]
    fn infer_action_case_insensitive_method() {
        assert_eq!(infer_action("post", "/api/v1/admin/users"), "user_create");
    }

    #[test]
    fn infer_action_trailing_slash_normalized() {
        assert_eq!(infer_action("POST", "/api/v1/admin/users/"), "user_create");
    }

    #[test]
    fn infer_action_unknown_falls_back() {
        assert_eq!(infer_action("POST", "/api/v1/unknown"), "post_request");
        assert_eq!(infer_action("DELETE", "/api/v1/foo/bar"), "delete_request");
    }

    #[test]
    fn build_filter_default_is_last_7_days() {
        let q = AuditLogQuery {
            from: None,
            to: None,
            user_id: None,
            username: None,
            action: None,
            page: None,
            page_size: None,
        };
        let now = 10_000_000_000i64;
        let f = build_filter(&q, now);
        assert_eq!(f.from, now - DEFAULT_WINDOW_MS);
        assert_eq!(f.to, now);
        assert_eq!(f.page, 1);
        assert_eq!(f.page_size, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn build_filter_respects_explicit_range() {
        let q = AuditLogQuery {
            from: Some(100),
            to: Some(200),
            user_id: Some("u1".to_string()),
            username: Some("alice".to_string()),
            action: Some("user_create".to_string()),
            page: Some(2),
            page_size: Some(50),
        };
        let f = build_filter(&q, 999);
        assert_eq!(f.from, 100);
        assert_eq!(f.to, 200);
        assert_eq!(f.page, 2);
        assert_eq!(f.page_size, 50);
        assert_eq!(f.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn build_filter_only_from_uses_now_as_to() {
        let q = AuditLogQuery {
            from: Some(500),
            to: None,
            user_id: None,
            username: None,
            action: None,
            page: None,
            page_size: None,
        };
        let f = build_filter(&q, 9999);
        assert_eq!(f.from, 500);
        assert_eq!(f.to, 9999);
    }

    #[test]
    fn build_filter_clamps_page_size() {
        let q = AuditLogQuery {
            from: None,
            to: None,
            user_id: None,
            username: None,
            action: None,
            page: Some(0),
            page_size: Some(9999),
        };
        let f = build_filter(&q, 0);
        assert_eq!(f.page, 1);
        assert_eq!(f.page_size, MAX_PAGE_SIZE);
    }
}
