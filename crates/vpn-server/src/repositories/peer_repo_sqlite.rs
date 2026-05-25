//! SQLite 实现的 PeerRepository（Epic 4：节点注册 / 心跳 / 注销 / 离线扫描）。

use chrono::Utc;
use sqlx::SqlitePool;
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
}
