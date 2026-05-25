//! SQLite 实现的 AuditLogRepository（Epic 5：审计日志写入 / 查询 / 清理）。

use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

/// 单条审计日志写入参数。
#[derive(Debug, Clone, Default)]
pub struct AuditLogEntry {
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub action: String,
    pub resource: String,
    pub ip_addr: Option<String>,
    pub user_agent: Option<String>,
    /// 结构化补充信息（JSON 文本）
    pub metadata: Option<String>,
    pub status_code: Option<i32>,
}

/// 审计日志行（与表列一一对应）。
#[derive(Debug, Clone)]
pub struct AuditLogRow {
    pub id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub action: String,
    pub resource: String,
    pub ip_addr: Option<String>,
    pub user_agent: Option<String>,
    pub metadata: Option<String>,
    pub status_code: Option<i32>,
    pub created_at: i64,
}

/// 数据库行元组（与 SELECT 列顺序一致），仅用于内部反序列化。
type AuditLogRowTuple = (
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
    i64,
);

impl From<AuditLogRowTuple> for AuditLogRow {
    fn from(r: AuditLogRowTuple) -> Self {
        AuditLogRow {
            id: r.0,
            user_id: r.1,
            username: r.2,
            action: r.3,
            resource: r.4,
            ip_addr: r.5,
            user_agent: r.6,
            metadata: r.7,
            status_code: r.8,
            created_at: r.9,
        }
    }
}

/// 列表查询过滤条件（已归一化：page/page_size 已套用默认值，时间窗已套用默认值）。
#[derive(Debug, Clone)]
pub struct AuditLogFilter {
    /// 起始时间（unix ms，含）
    pub from: i64,
    /// 结束时间（unix ms，含）
    pub to: i64,
    /// 精确匹配 user_id（None 表示不过滤）
    pub user_id: Option<String>,
    /// 模糊匹配 username（None 表示不过滤）
    pub username: Option<String>,
    /// 精确匹配 action（None 表示不过滤）
    pub action: Option<String>,
    pub page: u32,
    pub page_size: u32,
}

const SELECT_COLUMNS: &str = r#"id, user_id, username, action, resource, ip_addr,
                                user_agent, metadata, status_code, created_at"#;

#[derive(Debug, Clone)]
pub struct SqliteAuditLogRepository {
    pool: SqlitePool,
}

impl SqliteAuditLogRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 写入一条审计日志。
    pub async fn insert(&self, id: &str, entry: &AuditLogEntry, created_at: i64) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO audit_logs
               (id, user_id, username, action, resource, ip_addr, user_agent, metadata, status_code, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
        )
        .bind(id)
        .bind(&entry.user_id)
        .bind(&entry.username)
        .bind(&entry.action)
        .bind(&entry.resource)
        .bind(&entry.ip_addr)
        .bind(&entry.user_agent)
        .bind(&entry.metadata)
        .bind(entry.status_code)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    /// 统计符合过滤条件的总数（分页 total）。
    pub async fn count(&self, filter: &AuditLogFilter) -> Result<i64> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) FROM audit_logs");
        Self::push_where(&mut qb, filter);
        let count: (i64,) = qb
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(count.0)
    }

    /// 列表查询（时间窗 + 过滤 + 分页，按 created_at desc）。
    pub async fn list(&self, filter: &AuditLogFilter) -> Result<Vec<AuditLogRow>> {
        let page = filter.page.max(1);
        let page_size = filter.page_size.max(1);
        let offset = (page - 1) as i64 * page_size as i64;

        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new(format!("SELECT {SELECT_COLUMNS} FROM audit_logs"));
        Self::push_where(&mut qb, filter);
        qb.push(" ORDER BY created_at DESC LIMIT ");
        qb.push_bind(page_size as i64);
        qb.push(" OFFSET ");
        qb.push_bind(offset);

        let rows: Vec<AuditLogRowTuple> = qb
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(AuditLogRow::from).collect())
    }

    /// 删除 created_at < cutoff_ms 的日志（清理任务）。返回删除条数。
    pub async fn delete_older_than(&self, cutoff_ms: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM audit_logs WHERE created_at < ?1")
            .bind(cutoff_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 拼接 WHERE 子句（时间窗 + user_id / username / action）；绑定值用占位符防注入。
    fn push_where(qb: &mut QueryBuilder<Sqlite>, filter: &AuditLogFilter) {
        qb.push(" WHERE created_at >= ");
        qb.push_bind(filter.from);
        qb.push(" AND created_at <= ");
        qb.push_bind(filter.to);

        if let Some(user_id) = filter.user_id.as_deref().filter(|s| !s.is_empty()) {
            qb.push(" AND user_id = ");
            qb.push_bind(user_id.to_string());
        }
        if let Some(username) = filter.username.as_deref().filter(|s| !s.is_empty()) {
            qb.push(" AND username LIKE ");
            qb.push_bind(format!("%{}%", username));
        }
        if let Some(action) = filter.action.as_deref().filter(|s| !s.is_empty()) {
            qb.push(" AND action = ");
            qb.push_bind(action.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:audit_repo_test_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    fn entry(action: &str, username: Option<&str>) -> AuditLogEntry {
        AuditLogEntry {
            user_id: username.map(|_| "u1".to_string()),
            username: username.map(|s| s.to_string()),
            action: action.to_string(),
            resource: "/api/v1/admin/users".to_string(),
            ip_addr: Some("1.2.3.4".to_string()),
            user_agent: Some("agent".to_string()),
            metadata: None,
            status_code: Some(200),
        }
    }

    fn filter_default() -> AuditLogFilter {
        AuditLogFilter {
            from: 0,
            to: i64::MAX,
            user_id: None,
            username: None,
            action: None,
            page: 1,
            page_size: 20,
        }
    }

    #[tokio::test]
    async fn insert_and_list_roundtrip() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("user_create", Some("alice")), 1000)
            .await
            .unwrap();
        let rows = repo.list(&filter_default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "user_create");
        assert_eq!(rows[0].username.as_deref(), Some("alice"));
        assert_eq!(rows[0].status_code, Some(200));
        assert_eq!(repo.count(&filter_default()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_orders_by_created_at_desc() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("login_failed", None), 100)
            .await
            .unwrap();
        repo.insert("a2", &entry("user_create", Some("bob")), 200)
            .await
            .unwrap();
        let rows = repo.list(&filter_default()).await.unwrap();
        assert_eq!(rows[0].id, "a2");
        assert_eq!(rows[1].id, "a1");
    }

    #[tokio::test]
    async fn filter_by_time_window() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("x", None), 100).await.unwrap();
        repo.insert("a2", &entry("y", None), 5000).await.unwrap();
        let mut f = filter_default();
        f.from = 1000;
        f.to = 10000;
        let rows = repo.list(&f).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a2");
        assert_eq!(repo.count(&f).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn filter_by_action_and_username() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("user_create", Some("alice")), 100)
            .await
            .unwrap();
        repo.insert("a2", &entry("user_delete", Some("alicia")), 200)
            .await
            .unwrap();
        repo.insert("a3", &entry("user_create", Some("bob")), 300)
            .await
            .unwrap();

        let mut f = filter_default();
        f.action = Some("user_create".to_string());
        assert_eq!(repo.count(&f).await.unwrap(), 2);

        let mut f = filter_default();
        f.username = Some("ali".to_string());
        // 模糊匹配 alice + alicia
        assert_eq!(repo.count(&f).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn filter_by_user_id() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("x", Some("alice")), 100)
            .await
            .unwrap();
        let mut anon = entry("y", None);
        anon.user_id = None;
        repo.insert("a2", &anon, 200).await.unwrap();
        let mut f = filter_default();
        f.user_id = Some("u1".to_string());
        assert_eq!(repo.count(&f).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn delete_older_than_removes_old_rows() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        repo.insert("a1", &entry("x", None), 100).await.unwrap();
        repo.insert("a2", &entry("y", None), 5000).await.unwrap();
        let deleted = repo.delete_older_than(1000).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(repo.count(&filter_default()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn pagination_works() {
        let repo = SqliteAuditLogRepository::new(setup_pool().await);
        for i in 0..5 {
            repo.insert(&format!("a{i}"), &entry("x", None), i as i64)
                .await
                .unwrap();
        }
        let mut f = filter_default();
        f.page = 1;
        f.page_size = 2;
        assert_eq!(repo.list(&f).await.unwrap().len(), 2);
        f.page = 3;
        assert_eq!(repo.list(&f).await.unwrap().len(), 1);
        assert_eq!(repo.count(&f).await.unwrap(), 5);
    }
}
