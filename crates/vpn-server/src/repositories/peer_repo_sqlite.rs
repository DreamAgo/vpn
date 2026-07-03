//! SQLite 实现的 PeerRepository（Epic 4：节点注册 / 心跳 / 注销 / 离线扫描）。

use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

/// 数据库行（peers 表）。
#[derive(Debug, Clone, sqlx::FromRow)]
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
    /// 逗号分隔的 LAN 网段 CIDR（站点网关路由的内网），空串表示无。
    pub routed_subnets: String,
    /// 本次转为在线的起始时刻（unix ms）；不在线为 None（节点健康监控）。
    pub online_since: Option<i64>,
    /// 客户端最近上报的心跳往返延迟（毫秒）。
    pub rtt_ms: Option<i64>,
    /// 客户端最近上报的心跳丢包率（百分比 0-100）。
    pub loss_pct: Option<f64>,
    /// 客户端版本（注册时上报）。
    pub client_version: Option<String>,
}

const SELECT_COLUMNS: &str = r#"id, user_id, device_name, wg_public_key, vpn_ip, endpoint,
                                os_info, last_seen_at, status, created_at, updated_at, routed_subnets,
                                online_since, rtt_ms, loss_pct, client_version"#;

#[derive(Debug, Clone)]
pub struct SqlitePeerRepository {
    pool: SqlitePool,
}

impl SqlitePeerRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 该 user **最近注册/更新**的非 deleted peer。
    ///
    /// 多终端模式下一个 user 可有多个活跃 peer；本方法用于兼容旧的单终端语义
    /// （旧客户端心跳不带公钥、按用户下载配置等场景取"最新那台"）。
    pub async fn find_active_by_user(&self, user_id: &str) -> Result<Option<PeerRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM peers WHERE user_id = ?1 AND status != 'deleted'
             ORDER BY updated_at DESC LIMIT 1"
        );
        let row: Option<PeerRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row)
    }

    /// 该 user 的全部非 deleted peer（多终端模式；按创建时间升序）。
    pub async fn list_active_by_user(&self, user_id: &str) -> Result<Vec<PeerRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM peers WHERE user_id = ?1 AND status != 'deleted'
             ORDER BY created_at ASC"
        );
        let rows: Vec<PeerRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
    }

    /// 按 (user, wg_public_key) 查非 deleted peer（多终端模式下定位具体终端）。
    pub async fn find_active_by_user_and_pubkey(
        &self,
        user_id: &str,
        wg_public_key: &str,
    ) -> Result<Option<PeerRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM peers
             WHERE user_id = ?1 AND wg_public_key = ?2 AND status != 'deleted' LIMIT 1"
        );
        let row: Option<PeerRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(wg_public_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row)
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

    /// 列出活跃 peer 的 (wg_public_key, vpn_ip, routed_subnets)，用于启动时向内核接口恢复配置。
    pub async fn list_active_peer_keys(&self) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT wg_public_key, vpn_ip, routed_subnets FROM peers WHERE status NOT IN ('deleted', 'force_removed')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
    }

    /// 列出活跃且**声明了站点 LAN 网段**的网关 peer：(peer_id, user_id, routed_subnets)。
    ///
    /// 仅扫描 `routed_subnets != ''` 的网关 peer（绝大多数普通节点不声明网段），供
    /// allowed_routes 计算（每次心跳调用）与注册/改路由时的网段冲突检测复用，避免对
    /// 全量 peer 做全表扫描（参见 [`PeerService::compute_allowed_routes`]）。
    pub async fn list_active_gateway_routes(&self) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, user_id, routed_subnets FROM peers
             WHERE status NOT IN ('deleted', 'force_removed') AND routed_subnets != ''",
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
        client_version: Option<&str>,
        routed_subnets: &str,
    ) -> Result<PeerRow> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"INSERT INTO peers (id, user_id, device_name, wg_public_key, vpn_ip, os_info, client_version, routed_subnets, status, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'offline', ?9, ?9)"#,
        )
        .bind(id)
        .bind(user_id)
        .bind(device_name)
        .bind(wg_public_key)
        .bind(vpn_ip)
        .bind(os_info)
        .bind(client_version)
        .bind(routed_subnets)
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
                routed_subnets: routed_subnets.to_string(),
                online_since: None,
                rtt_ms: None,
                loss_pct: None,
                client_version: client_version.map(|s| s.to_string()),
            }),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => Err(
                AppError::DuplicateResource("WireGuard 公钥或 VPN IP".to_string()),
            ),
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 复用既有 peer：更新 wg_public_key / device_name / os_info（保留 vpn_ip）。
    /// wg_public_key 与别的 peer 冲突返回 DuplicateResource。
    ///
    /// 「强制下线」语义=踢下线 + 强制重连:若该 peer 当前是 `force_removed`,重新注册
    /// 时把状态清回 `offline`(下次心跳即恢复 `online`),使节点重连后可再次上线。
    /// 其它状态(online/offline)保持不变。永久封禁应禁用/删除用户账号。
    #[allow(clippy::too_many_arguments)]
    pub async fn update_registration(
        &self,
        id: &str,
        device_name: &str,
        wg_public_key: &str,
        os_info: Option<&str>,
        client_version: Option<&str>,
        routed_subnets: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"UPDATE peers SET device_name = ?1, wg_public_key = ?2, os_info = ?3, routed_subnets = ?4, updated_at = ?5,
                   client_version = COALESCE(?7, client_version),
                   status = CASE WHEN status = 'force_removed' THEN 'offline' ELSE status END
               WHERE id = ?6"#,
        )
        .bind(device_name)
        .bind(wg_public_key)
        .bind(os_info)
        .bind(routed_subnets)
        .bind(now)
        .bind(id)
        .bind(client_version)
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

    /// 心跳（按终端）：更新指定 peer 的 last_seen_at / status='online' / endpoint（若有）
    /// 及健康指标（rtt/loss，客户端上报；None 保留原值）。
    /// online_since：由非在线转为在线时记为本次时刻（在线时长起点），持续在线则保留。
    /// 返回受影响行数（0 表示 peer 不存在或已 deleted）。
    pub async fn touch_heartbeat_by_id(
        &self,
        peer_id: &str,
        endpoint: Option<&str>,
        rtt_ms: Option<i64>,
        loss_pct: Option<f64>,
        now_ms: i64,
    ) -> Result<u64> {
        // endpoint / rtt / loss 为 None 时保留原值（COALESCE）。
        let result = sqlx::query(
            r#"UPDATE peers
               SET last_seen_at = ?1, status = 'online', endpoint = COALESCE(?2, endpoint),
                   rtt_ms = COALESCE(?4, rtt_ms), loss_pct = COALESCE(?5, loss_pct),
                   online_since = CASE WHEN status = 'online' AND online_since IS NOT NULL
                                       THEN online_since ELSE ?1 END,
                   updated_at = ?1
               WHERE id = ?3 AND status != 'deleted'"#,
        )
        .bind(now_ms)
        .bind(endpoint)
        .bind(peer_id)
        .bind(rtt_ms)
        .bind(loss_pct)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 心跳（按用户，旧客户端兼容路径）：更新该 user 全部活跃 peer 的
    /// last_seen_at / status='online' / endpoint（若有）。
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
               SET last_seen_at = ?1, status = 'online', endpoint = COALESCE(?2, endpoint),
                   online_since = CASE WHEN status = 'online' AND online_since IS NOT NULL
                                       THEN online_since ELSE ?1 END,
                   updated_at = ?1
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
            "UPDATE peers SET status = 'deleted', online_since = NULL, updated_at = ?1 WHERE user_id = ?2 AND status != 'deleted'",
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
            r#"UPDATE peers SET status = 'offline', online_since = NULL, updated_at = ?1
               WHERE status = 'online' AND last_seen_at IS NOT NULL AND last_seen_at < ?2"#,
        )
        .bind(now)
        .bind(cutoff_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 列出即将被离线扫描标记为 offline 的站点网关。
    pub async fn list_stale_online_gateways(&self, cutoff_ms: i64) -> Result<Vec<PeerRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM peers
             WHERE status = 'online'
               AND last_seen_at IS NOT NULL
               AND last_seen_at < ?1
               AND routed_subnets != ''
             ORDER BY last_seen_at ASC"
        );
        let rows = sqlx::query_as(&sql)
            .bind(cutoff_ms)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
    }

    /// 按 id 查询 peer（Story 5.5：admin 强制下线前定位）。
    pub async fn find_by_id(&self, id: &str) -> Result<Option<PeerRow>> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM peers WHERE id = ?1");
        let row: Option<PeerRow> = sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row)
    }

    /// 更新指定 peer 的 routed_subnets（异地组网网段编辑）。返回受影响行数。
    pub async fn update_routed_subnets(&self, id: &str, routed_subnets: &str) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result =
            sqlx::query("UPDATE peers SET routed_subnets = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(routed_subnets)
                .bind(now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// Story 5.5：把指定 peer 标记为 'force_removed'。返回受影响行数。
    pub async fn mark_force_removed(&self, id: &str) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result =
            sqlx::query("UPDATE peers SET status = 'force_removed', online_since = NULL, updated_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 硬删除：从 peers 表彻底删除指定行（admin 彻底删除节点）。返回受影响行数。
    ///
    /// 与 `mark_force_removed` / `mark_deleted_by_user` 的软删除不同，这里物理删除记录，
    /// 调用方需同时摘除 WireGuard peer 并回收 VPN IP。
    pub async fn delete_by_id(&self, id: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM peers WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 硬删某用户的**全部** peer 行（任意状态，含历史 'deleted' 行），返回被删行的 vpn_ip
    /// 以便调用方回收 IP。删除用户前调用以满足 `peers.user_id -> users.id` 外键（无级联）。
    pub async fn delete_all_by_user(&self, user_id: &str) -> Result<Vec<String>> {
        let ips: Vec<(String,)> = sqlx::query_as("SELECT vpn_ip FROM peers WHERE user_id = ?1")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        sqlx::query("DELETE FROM peers WHERE user_id = ?1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(ips.into_iter().map(|r| r.0).collect())
    }

    /// Story 5.5：admin peer 列表（JOIN users 取 username/email）。
    /// 按 last_seen_at desc（NULL 最后），search 模糊匹配 username/device_name，status 精确筛选。
    pub async fn list_admin(&self, filter: &AdminPeerFilter) -> Result<Vec<AdminPeerRow>> {
        let page = filter.page.max(1);
        let page_size = filter.page_size.max(1);
        let offset = (page - 1) as i64 * page_size as i64;

        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            r#"SELECT p.id, p.user_id, u.username, u.email, p.device_name, p.wg_public_key,
                      p.vpn_ip, p.endpoint, p.os_info, p.last_seen_at, p.status, p.created_at, p.routed_subnets,
                      p.online_since, p.rtt_ms, p.loss_pct, p.client_version
               FROM peers p JOIN users u ON p.user_id = u.id"#,
        );
        Self::push_admin_where(&mut qb, filter);
        // NULL last_seen_at 排最后：先按 IS NULL 升序，再按值降序。
        qb.push(" ORDER BY (p.last_seen_at IS NULL) ASC, p.last_seen_at DESC LIMIT ");
        qb.push_bind(page_size as i64);
        qb.push(" OFFSET ");
        qb.push_bind(offset);

        let rows: Vec<AdminPeerRow> = qb
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
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
#[derive(Debug, Clone, sqlx::FromRow)]
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
    pub routed_subnets: String,
    pub online_since: Option<i64>,
    pub rtt_ms: Option<i64>,
    pub loss_pct: Option<f64>,
    pub client_version: Option<String>,
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
            .insert(
                "p1",
                "user-1",
                "MBP",
                "PK1",
                "10.8.0.2",
                Some("macOS"),
                None,
                "",
            )
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        let err = repo
            .insert("p2", "user-1", "Other", "PK1", "10.8.0.3", None, None, "")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn duplicate_vpn_ip_returns_error() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        let err = repo
            .insert("p2", "user-1", "Other", "PK2", "10.8.0.2", None, None, "")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn update_registration_preserves_ip() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        repo.update_registration("p1", "MBP2", "PK2", Some("linux"), None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        repo.mark_deleted_by_user("user-1").await.unwrap();
        assert!(repo.list_active_vpn_ips().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn heartbeat_sets_online_and_endpoint() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        let affected = repo.mark_deleted_by_user("user-1").await.unwrap();
        assert_eq!(affected, 1);
        assert!(repo.find_active_by_user("user-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mark_stale_offline_only_old_online_peers() {
        let repo = SqlitePeerRepository::new(setup_pool().await);
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
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
        repo.insert("p1", "user-1", "Laptop", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        repo.insert("p2", "user-2", "Phone", "PK2", "10.8.0.3", None, None, "")
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
        repo.insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, None, "")
            .await
            .unwrap();
        repo.insert("p2", "user-2", "Phone", "PK2", "10.8.0.3", None, None, "")
            .await
            .unwrap();
        repo.touch_heartbeat("user-2", None, 5000).await.unwrap();
        let rows = repo.list_admin(&admin_filter_default()).await.unwrap();
        // p2 (has last_seen) 排前，p1 (NULL) 排后
        assert_eq!(rows[0].id, "p2");
        assert_eq!(rows[1].id, "p1");
    }
}
