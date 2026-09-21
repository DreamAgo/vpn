//! 审计日志业务服务（Epic 5：写入 / 查询 / 清理）。
//!
//! 写入是「尽力而为」：失败仅降级为 tracing::warn，不阻塞主请求路径。

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
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
    failed_writes: Arc<AtomicU64>,
    dropped_events: Arc<AtomicU64>,
    writers: Arc<tokio::sync::Semaphore>,
}

impl AuditService {
    pub fn new(repo: SqliteAuditLogRepository) -> Self {
        Self {
            repo,
            failed_writes: Arc::new(AtomicU64::new(0)),
            dropped_events: Arc::new(AtomicU64::new(0)),
            writers: Arc::new(tokio::sync::Semaphore::new(16)),
        }
    }

    pub async fn actor_name(&self, id: &str) -> Option<String> {
        self.repo.actor_name(id).await.ok().flatten()
    }
    pub fn health(&self) -> serde_json::Value {
        serde_json::json!({"failed_writes":self.failed_writes.load(Ordering::Relaxed),"failed_transaction_writes":crate::middleware::audit_context::FAILED_TRANSACTION_WRITES.load(Ordering::Relaxed),"dropped_events":self.dropped_events.load(Ordering::Relaxed)})
    }
    /// Only fallback/auth events are best effort; critical changes use the business transaction.
    pub async fn log(&self, entry: AuditLogEntry, now_ms: i64) {
        let Ok(_permit) = self.writers.try_acquire() else {
            let count = self.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_power_of_two() {
                tracing::warn!(dropped_events = count, "审计事件并发写入已达上限");
            }
            return;
        };
        let id = Uuid::now_v7().to_string();
        if let Err(e) = self.repo.insert(&id, &entry, now_ms).await {
            self.failed_writes.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = ?e, action = %entry.action, "审计日志写入失败（已降级，不影响主流程）");
        }
    }

    pub async fn log_auth(
        &self,
        mut entry: AuditLogEntry,
        success: bool,
        error: Option<&vpn_core::AppError>,
        request_id: Option<&str>,
        now: i64,
    ) {
        entry.status_code = Some(
            error
                .map(|e| crate::error::status_code(e).as_u16() as i32)
                .unwrap_or(200),
        );
        let reason_code = error.map(|e| e.code());
        entry.metadata = Some(serde_json::json!({"outcome":if success {"success"}else{"failed"},
            "reason_code":reason_code,"request_id":request_id.map(|v|v.chars().take(128).collect::<String>()).unwrap_or_else(||Uuid::now_v7().to_string())}).to_string());
        self.log(entry, now).await;
    }

    /// 删除早于 cutoff_ms 的日志。返回删除条数。
    pub async fn purge_older_than(&self, cutoff_ms: i64) -> Result<u64> {
        self.repo.delete_older_than(cutoff_ms).await
    }

    /// Story 5.4：分页查询审计日志。
    ///
    /// 不传 from/to 时默认返回最近 7 天（now - 7d ..= now）。
    pub async fn query(&self, query: &AuditLogQuery, now_ms: i64) -> Result<Page<AuditLogDto>> {
        if query.from.zip(query.to).is_some_and(|(f, t)| f > t)
            || query
                .outcome
                .as_deref()
                .is_some_and(|v| !matches!(v, "success" | "failed"))
            || query
                .category
                .as_deref()
                .is_some_and(|v| !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err(vpn_core::AppError::Validation("审计筛选条件无效".into()));
        }
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
        (None, Some(t)) => (t.saturating_sub(DEFAULT_WINDOW_MS), t),
        (None, None) => (now_ms.saturating_sub(DEFAULT_WINDOW_MS), now_ms),
    };
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    AuditLogFilter {
        resource: query.resource.clone(),
        outcome: query.outcome.clone(),
        category: query.category.clone(),
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
        ("PUT", "/api/v1/admin/network/settings") => "network.settings.update".into(),
        ("PUT", "/api/v1/admin/system/routes") => "network.routes.update".into(),
        ("PUT", "/api/v1/admin/integrations/settings") => "integration.settings.update".into(),
        ("PUT", "/api/v1/admin/notifications/email") => "notification.settings.update".into(),
        ("POST", "/api/v1/admin/notifications/email/test") => "notification.test".into(),
        ("POST", "/api/v1/admin/system/restart") => "system.restart.request".into(),
        ("GET", "/api/v1/admin/backup") => "backup.download".into(),
        ("POST", "/api/v1/admin/backup/restore") => "backup.restore".into(),
        ("PUT", "/api/v1/admin/client-updates") => "client_update.configure".into(),
        ("POST", "/api/v1/admin/client-updates/sync") => "client_update.sync".into(),
        ("POST", "/api/v1/admin/integrations/feishu/approval-subscription") => {
            "integration.approval.subscribe".into()
        }
        ("POST", "/api/v1/admin/integrations/feishu/users/lookup") => {
            "integration.user.lookup".into()
        }
        ("POST", "/api/v1/admin/integrations/feishu/users/sync") => "integration.user.sync".into(),
        ("POST", "/api/v1/admin/api-keys") => "api_key.create".into(),
        ("POST", "/api/v1/admin/groups") => "group.create".into(),
        ("POST", "/api/v1/admin/subnets") => "subnet.create".into(),
        ("POST", "/api/v1/admin/users") => "user_create".to_string(),
        ("POST", "/api/v1/auth/first-time-setup") => "first_time_setup".to_string(),
        ("POST", "/api/v1/auth/login") => "login".to_string(),
        ("POST", "/api/v1/auth/logout") => "logout".to_string(),
        ("POST", "/api/v1/auth/change-password") => "change_password".to_string(),
        ("POST", "/api/v1/peers/register") => "peer_register".to_string(),
        ("POST", "/api/v1/peers/heartbeat") => "peer_heartbeat".to_string(),
        ("DELETE", "/api/v1/peers/me") => "peer_delete".to_string(),
        _ => {
            if m == "PUT" && p.starts_with("/api/v1/admin/users/") && p.ends_with("/groups") {
                return "user.groups.update".into();
            }
            if m == "PATCH" && p.contains("/approval-grants/") {
                return "grant.expiry.update".into();
            }
            for (prefix, entity) in [
                ("/api/v1/admin/api-keys/", "api_key"),
                ("/api/v1/admin/groups/", "group"),
                ("/api/v1/admin/subnets/", "subnet"),
            ] {
                if p.strip_prefix(prefix)
                    .is_some_and(|id| !id.is_empty() && !id.contains('/'))
                {
                    return format!(
                        "{entity}.{}",
                        if m == "DELETE" { "delete" } else { "update" }
                    );
                }
            }
            if m == "PATCH" && is_admin_peer_id(p) {
                return "peer.routes.update".into();
            }
            if m == "DELETE" && p.starts_with("/api/v1/admin/peers/") && p.ends_with("/purge") {
                return "peer.purge".into();
            }
            if m == "POST" && p.starts_with("/api/v1/admin/users/") {
                if p.ends_with("/feishu-binding") {
                    return "user.feishu.bind".into();
                }
                if p.ends_with("/feishu-sync") {
                    return "user.feishu.sync".into();
                }
            }
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
            resource: None,
            outcome: None,
            category: None,
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
            resource: None,
            outcome: None,
            category: None,
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
            resource: None,
            outcome: None,
            category: None,
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
            resource: None,
            outcome: None,
            category: None,
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
