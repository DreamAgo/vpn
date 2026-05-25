//! SQLite 实现的 PeerRepository（Epic 4：节点注册 / 心跳 / 注销 / 离线扫描）。

use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

/// 数据库行（peers 表）。
#[derive(Debug, Clone)]
pub struct PeerRow {
    pub id: String,
    pub user_id: String,
    pub device_name: String,
    pub wg_public_key: String,
    pub vpn_ip: String,
    pub endpoint: Option<String>,
    pub os_info: Option<String>,
    pub last_seen_at: Option<i64>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 数据库行元组（与 SELECT 列顺序一致），仅用于内部反序列化。
type PeerRowTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
    i64,
    i64,
);

impl From<PeerRowTuple> for PeerRow {
    fn from(r: PeerRowTuple) -> Self {
        PeerRow {
            id: r.0,
            user_id: r.1,
            device_name: r.2,
            wg_public_key: r.3,
            vpn_ip: r.4,
            endpoint: r.5,
            os_info: r.6,
            last_seen_at: r.7,
            status: r.8,
            created_at: r.9,
            updated_at: r.10,
        }
    }
}

const SELECT_COLUMNS: &str = r#"id, user_id, device_name, wg_public_key, vpn_ip, endpoint,
                                os_info, last_seen_at, status, created_at, updated_at"#;

#[derive(Debug, Clone)]
pub struct SqlitePeerRepository {
    pool: SqlitePool,
}

impl SqlitePeerRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 该 user 当前的非 deleted peer（业务约束：一个 user 最多一个活跃 peer）。
    pub async fn find_active_by_user(&self, user_id: &str) -> Result<Option<PeerRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM peers WHERE user_id = ?1 AND status != 'deleted' LIMIT 1"
        );
        let row: Option<PeerRowTuple> = sqlx::query_as(&sql)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(PeerRow::from))
    }

    /// 所有非 deleted peer 的 vpn_ip（启动时回填 IpPool）。
    pub async fn list_active_vpn_ips(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT vpn_ip FROM peers WHERE status != 'deleted'")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// 列出活跃 peer 的 (wg_public_key, vpn_ip)，用于启动时向内核接口恢复配置。
    pub async fn list_active_peer_keys(&self) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT wg_public_key, vpn_ip FROM peers WHERE status NOT IN ('deleted', 'force_removed')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
    }

    /// 插入新 peer。wg_public_key / vpn_ip 冲突返回 DuplicateResource。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        id: &str,
        user_id: &str,
        device_name: &str,
        wg_public_key: &str,
        vpn_ip: &str,
        os_info: Option<&str>,
    ) -> Result<PeerRow> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"INSERT INTO peers (id, user_id, device_name, wg_public_key, vpn_ip, os_info, status, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'offline', ?7, ?7)"#,
        )
        .bind(id)
        .bind(user_id)
        .bind(device_name)
        .bind(wg_public_key)
        .bind(vpn_ip)
        .bind(os_info)
        .bind(now)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(PeerRow {
                id: id.to_string(),
                user_id: user_id.to_string(),
                device_name: device_name.to_string(),
                wg_public_key: wg_public_key.to_string(),
                vpn_ip: vpn_ip.to_string(),
                endpoint: None,
                os_info: os_info.map(|s| s.to_string()),
                last_seen_at: None,
                status: "offline".to_string(),
                created_at: now,
                updated_at: now,
            }),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => Err(
                AppError::DuplicateResource("WireGuard 公钥或 VPN IP".to_string()),
            ),
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 复用既有 peer：更新 wg_public_key / device_name / os_info（保留 vpn_ip）。
    /// wg_public_key 与别的 peer 冲突返回 DuplicateResource。
    pub async fn update_registration(
        &self,
        id: &str,
        device_name: &str,
        wg_public_key: &str,
        os_info: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"UPDATE peers SET device_name = ?1, wg_public_key = ?2, os_info = ?3, updated_at = ?4
               WHERE id = ?5"#,
        )
        .bind(device_name)
        .bind(wg_public_key)
        .bind(os_info)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(AppError::DuplicateResource("WireGuard 公钥".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 心跳：更新该 user 活跃 peer 的 last_seen_at / status='online' / endpoint（若有）。
    /// 返回受影响行数（0 表示该 user 无活跃 peer）。
    pub async fn touch_heartbeat(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
    ) -> Result<u64> {
        // endpoint 为 None 时保留原值（COALESCE）。
        let result = sqlx::query(
            r#"UPDATE peers
               SET last_seen_at = ?1, status = 'online', endpoint = COALESCE(?2, endpoint), updated_at = ?1
               WHERE user_id = ?3 AND status != 'deleted'"#,
        )
        .bind(now_ms)
        .bind(endpoint)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 注销该 user 的活跃 peer（status='deleted'）。返回受影响行数。
    /// 保留记录与 vpn_ip；IP 释放交由后续清理任务处理。
    // TODO(Epic 4): 增加清理任务，对 deleted 超过 24h 的 peer 释放其 vpn_ip 回 IpPool。
    pub async fn mark_deleted_by_user(&self, user_id: &str) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            "UPDATE peers SET status = 'deleted', updated_at = ?1 WHERE user_id = ?2 AND status != 'deleted'",
        )
        .bind(now)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 离线检测：把 last_seen_at < cutoff 且 status='online' 的 peer 标记 offline。
    /// 返回标记的行数。
    pub async fn mark_stale_offline(&self, cutoff_ms: i64) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"UPDATE peers SET status = 'offline', updated_at = ?1
               WHERE status = 'online' AND last_seen_at IS NOT NULL AND last_seen_at < ?2"#,
        )
        .bind(now)
        .bind(cutoff_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 按 id 查询 peer（Story 5.5：admin 强制下线前定位）。
    pub async fn find_by_id(&self, id: &str) -> Result<Option<PeerRow>> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM peers WHERE id = ?1");
        let row: Option<PeerRowTuple> = sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(PeerRow::from))
    }

    /// Story 5.5：把指定 peer 标记为 'force_removed'。返回受影响行数。
    pub async fn mark_force_removed(&self, id: &str) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result =
            sqlx::query("UPDATE peers SET status = 'force_removed', updated_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// Story 5.5：admin peer 列表（JOIN users 取 username/email）。
    /// 按 last_seen_at desc（NULL 最后），search 模糊匹配 username/device_name，status 精确筛选。
    pub async fn list_admin(&self, filter: &AdminPeerFilter) -> Result<Vec<AdminPeerRow>> {
        let page = filter.page.max(1);
        let page_size = filter.page_size.max(1);
        let offset = (page - 1) as i64 * page_size as i64;

        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            r#"SELECT p.id, p.user_id, u.username, u.email, p.device_name, p.wg_public_key,
                      p.vpn_ip, p.endpoint, p.os_info, p.last_seen_at, p.status, p.created_at
               FROM peers p JOIN users u ON p.user_id = u.id"#,
        );
        Self::push_admin_where(&mut qb, filter);
        // NULL last_seen_at 排最后：先按 IS NULL 升序，再按值降序。
        qb.push(" ORDER BY (p.last_seen_at IS NULL) ASC, p.last_seen_at DESC LIMIT ");
        qb.push_bind(page_size as i64);
        qb.push(" OFFSET ");
        qb.push_bind(offset);

        let rows: Vec<AdminPeerRowTuple> = qb
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(AdminPeerRow::from).collect())
    }

    /// admin peer 列表总数（用于分页 total）。
    pub async fn count_admin(&self, filter: &AdminPeerFilter) -> Result<i64> {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT COUNT(*) FROM peers p JOIN users u ON p.user_id = u.id");
        Self::push_admin_where(&mut qb, filter);
        let count: (i64,) = qb
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(count.0)
    }

    /// 拼接 admin 列表 WHERE 子句（search / status）；绑定值用占位符防注入。
    fn push_admin_where(qb: &mut QueryBuilder<Sqlite>, filter: &AdminPeerFilter) {
        let mut first = true;
        let mut clause = |qb: &mut QueryBuilder<Sqlite>| {
            if first {
                qb.push(" WHERE ");
                first = false;
            } else {
                qb.push(" AND ");
            }
        };

        if let Some(search) = filter.search.as_deref().filter(|s| !s.is_empty()) {
            clause(qb);
            let pattern = format!("%{}%", search);
            qb.push("(u.username LIKE ");
            qb.push_bind(pattern.clone());
            qb.push(" OR p.device_name LIKE ");
            qb.push_bind(pattern);
            qb.push(")");
        }

        if let Some(status) = filter.status.as_deref().filter(|s| !s.is_empty()) {
            clause(qb);
            qb.push("p.status = ");
            qb.push_bind(status.to_string());
        }
    }
}

/// admin peer 列表过滤条件（已归一化：page/page_size 已套默认值）。
#[derive(Debug, Clone)]
pub struct AdminPeerFilter {
    /// 模糊匹配 username / device_name（None 表示不过滤）
    pub search: Option<String>,
    /// 状态精确筛选（None 表示不过滤）
    pub status: Option<String>,
    pub page: u32,
    pub page_size: u32,
}

/// admin peer 列表行（peers JOIN users）。
#[derive(Debug, Clone)]
pub struct AdminPeerRow {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub device_name: String,
    pub wg_public_key: String,
    pub vpn_ip: String,
    pub endpoint: Option<String>,
    pub os_info: Option<String>,
    pub last_seen_at: Option<i64>,
    pub status: String,
    pub created_at: i64,
}

/// admin 列表行元组（与 SELECT 列顺序一致）。
type AdminPeerRowTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
    i64,
);

impl From<AdminPeerRowTuple> for AdminPeerRow {
    fn from(r: AdminPeerRowTuple) -> Self {
        AdminPeerRow {
            id: r.0,
            user_id: r.1,
            username: r.2,
            email: r.3,
            device_name: r.4,
            wg_public_key: r.5,
            vpn_ip: r.6,
            endpoint: r.7,
            os_info: r.8,
            last_seen_at: r.9,
            status: r.10,
            created_at: r.11,
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
            "sqlite:file:peer_repo_test_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        // peers.user_id FK -> users，插入一个用户。
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-1', 'alice', 'a@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn insert_and_find_active_by_user() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        let row = repo
            .insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", Some("macOS"))
            .await
            .unwrap();
        assert_eq!(row.status, "offline");
        let found = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(found.id, "p1");
        assert_eq!(found.vpn_ip, "10.8.0.2");
        assert_eq!(found.os_info.as_deref(), Some("macOS"));
    }

    #[tokio::test]
    async fn duplicate_public_key_returns_error() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let err = repo
            .insert("p2", "user-1", "Other", "PK1", "10.8.0.3", None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn duplicate_vpn_ip_returns_error() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let err = repo
            .insert("p2", "user-1", "Other", "PK2", "10.8.0.2", None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn update_registration_preserves_ip() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        repo.update_registration("p1", "MBP2", "PK2", Some("linux"))
            .await
            .unwrap();
        let row = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(row.vpn_ip, "10.8.0.2");
        assert_eq!(row.wg_public_key, "PK2");
        assert_eq!(row.device_name, "MBP2");
        assert_eq!(row.os_info.as_deref(), Some("linux"));
    }

    #[tokio::test]
    async fn list_active_vpn_ips_excludes_deleted() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        repo.mark_deleted_by_user("user-1").await.unwrap();
        assert!(repo.list_active_vpn_ips().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn heartbeat_sets_online_and_endpoint() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let affected = repo
            .touch_heartbeat("user-1", Some("1.2.3.4:51820"), 1000)
            .await
            .unwrap();
        assert_eq!(affected, 1);
        let row = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(row.status, "online");
        assert_eq!(row.last_seen_at, Some(1000));
        assert_eq!(row.endpoint.as_deref(), Some("1.2.3.4:51820"));

        // endpoint=None 保留原值
        repo.touch_heartbeat("user-1", None, 2000).await.unwrap();
        let row = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(row.endpoint.as_deref(), Some("1.2.3.4:51820"));
        assert_eq!(row.last_seen_at, Some(2000));
    }

    #[tokio::test]
    async fn heartbeat_unknown_user_affects_zero() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        let affected = repo.touch_heartbeat("user-1", None, 1000).await.unwrap();
        assert_eq!(affected, 0);
    }

    #[tokio::test]
    async fn mark_deleted_removes_from_active() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let affected = repo.mark_deleted_by_user("user-1").await.unwrap();
        assert_eq!(affected, 1);
        assert!(repo.find_active_by_user("user-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mark_stale_offline_only_old_online_peers() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        // last_seen_at = 100 (old), online
        repo.touch_heartbeat("user-1", None, 100).await.unwrap();
        // cutoff = 1000 → p1 应被标 offline
        let marked = repo.mark_stale_offline(1000).await.unwrap();
        assert_eq!(marked, 1);
        let row = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(row.status, "offline");

        // 再扫一次：已 offline 不再受影响
        let marked = repo.mark_stale_offline(1000).await.unwrap();
        assert_eq!(marked, 0);
    }

    #[tokio::test]
    async fn mark_stale_offline_skips_recent_peers() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        repo.touch_heartbeat("user-1", None, 5000).await.unwrap();
        // cutoff = 1000 → last_seen 5000 > 1000，不标记
        let marked = repo.mark_stale_offline(1000).await.unwrap();
        assert_eq!(marked, 0);
        let row = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn find_by_id_and_mark_force_removed() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let found = repo.find_by_id("p1").await.unwrap().unwrap();
        assert_eq!(found.wg_public_key, "PK1");
        assert!(repo.find_by_id("missing").await.unwrap().is_none());

        let affected = repo.mark_force_removed("p1").await.unwrap();
        assert_eq!(affected, 1);
        let row = repo.find_by_id("p1").await.unwrap().unwrap();
        assert_eq!(row.status, "force_removed");
        // find_active_by_user 仅排除 'deleted'，force_removed 仍可被查到——
        // 这是 heartbeat_checked 据以拒绝心跳的依据。
        let active = repo.find_active_by_user("user-1").await.unwrap().unwrap();
        assert_eq!(active.status, "force_removed");
    }

    fn admin_filter_default() -> AdminPeerFilter {
        AdminPeerFilter {
            search: None,
            status: None,
            page: 1,
            page_size: 20,
        }
    }

    #[tokio::test]
    async fn list_admin_joins_username_and_email() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let rows = repo.list_admin(&admin_filter_default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "alice");
        assert_eq!(rows[0].email, "a@e.com");
        assert_eq!(rows[0].device_name, "MBP");
        assert_eq!(repo.count_admin(&admin_filter_default()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_admin_search_and_status_filter() {
        let pool = setup_pool().await;
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-2', 'bob', 'b@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let repo = SqlitePeerRepository::new(pool);
        repo.insert("p1", "user-1", "Laptop", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        repo.insert("p2", "user-2", "Phone", "PK2", "10.8.0.3", None)
            .await
            .unwrap();

        // search by username
        let mut f = admin_filter_default();
        f.search = Some("alice".to_string());
        assert_eq!(repo.count_admin(&f).await.unwrap(), 1);

        // search by device_name
        let mut f = admin_filter_default();
        f.search = Some("Phone".to_string());
        let rows = repo.list_admin(&f).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "bob");

        // status filter
        repo.mark_force_removed("p1").await.unwrap();
        let mut f = admin_filter_default();
        f.status = Some("force_removed".to_string());
        assert_eq!(repo.count_admin(&f).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_admin_orders_null_last_seen_last() {
        let pool = setup_pool().await;
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-2', 'bob', 'b@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let repo = SqlitePeerRepository::new(pool);
        // p1: no heartbeat (NULL last_seen), p2: has heartbeat
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        repo.insert("p2", "user-2", "Phone", "PK2", "10.8.0.3", None)
            .await
            .unwrap();
        repo.touch_heartbeat("user-2", None, 5000).await.unwrap();
        let rows = repo.list_admin(&admin_filter_default()).await.unwrap();
        // p2 (has last_seen) 排前，p1 (NULL) 排后
        assert_eq!(rows[0].id, "p2");
        assert_eq!(rows[1].id, "p1");
    }
}
