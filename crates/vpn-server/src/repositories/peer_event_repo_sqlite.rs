//! SQLite 实现的节点变更记录仓库（节点健康监控：OS / IP / Endpoint 等变化历史）。

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;
use vpn_core::{AppError, Result};

/// 变更记录视图行（LEFT JOIN peers/users 解析设备名与用户名；
/// peer 或用户被物理删除后记录仍在，名称为 None）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeerEventViewRow {
    pub id: String,
    pub peer_id: String,
    pub device_name: Option<String>,
    pub username: Option<String>,
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct SqlitePeerEventRepository {
    pool: SqlitePool,
}

impl SqlitePeerEventRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 追加一条变更记录。field 取值：'os_info' | 'endpoint' | 'vpn_ip' |
    /// 'device_name' | 'client_version'。
    pub async fn insert(
        &self,
        peer_id: &str,
        user_id: &str,
        field: &str,
        old_value: Option<&str>,
        new_value: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            r#"INSERT INTO peer_events (id, peer_id, user_id, field, old_value, new_value, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        )
        .bind(Uuid::now_v7().to_string())
        .bind(peer_id)
        .bind(user_id)
        .bind(field)
        .bind(old_value)
        .bind(new_value)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    /// 最近的变更记录（时间倒序）。`peer_id` 为 None 时跨全部节点。
    pub async fn list(&self, peer_id: Option<&str>, limit: u32) -> Result<Vec<PeerEventViewRow>> {
        let base = r#"SELECT e.id, e.peer_id, p.device_name, u.username,
                             e.field, e.old_value, e.new_value, e.created_at
                      FROM peer_events e
                      LEFT JOIN peers p ON e.peer_id = p.id
                      LEFT JOIN users u ON e.user_id = u.id"#;
        let rows: Vec<PeerEventViewRow> = match peer_id {
            Some(pid) => {
                let sql =
                    format!("{base} WHERE e.peer_id = ?1 ORDER BY e.created_at DESC LIMIT ?2");
                sqlx::query_as(&sql)
                    .bind(pid)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await
            }
            None => {
                let sql = format!("{base} ORDER BY e.created_at DESC LIMIT ?1");
                sqlx::query_as(&sql)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await
            }
        }
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows)
    }

    /// 清理早于 cutoff（unix ms）的历史记录，返回删除行数（保留期治理）。
    pub async fn prune_before(&self, cutoff_ms: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM peer_events WHERE created_at < ?1")
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
            "sqlite:file:peer_event_repo_test_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-1', 'alice', 'a@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO peers (id, user_id, device_name, wg_public_key, vpn_ip, routed_subnets, status, created_at, updated_at)
               VALUES ('p1', 'user-1', 'MBP', 'PK1', '10.8.0.2', '', 'offline', 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn insert_and_list_resolves_names() {
        let repo = SqlitePeerEventRepository::new(setup_pool().await);
        repo.insert(
            "p1",
            "user-1",
            "endpoint",
            Some("1.1.1.1:1"),
            Some("2.2.2.2:2"),
        )
        .await
        .unwrap();
        repo.insert("p1", "user-1", "os_info", None, Some("macOS 15"))
            .await
            .unwrap();

        let all = repo.list(None, 50).await.unwrap();
        assert_eq!(all.len(), 2);
        // 时间倒序：后插入的在前。
        assert_eq!(all[0].field, "os_info");
        assert_eq!(all[0].device_name.as_deref(), Some("MBP"));
        assert_eq!(all[0].username.as_deref(), Some("alice"));

        let filtered = repo.list(Some("p1"), 1).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(repo.list(Some("missing"), 50).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_survives_peer_deletion() {
        let pool = setup_pool().await;
        let repo = SqlitePeerEventRepository::new(pool.clone());
        repo.insert("p1", "user-1", "vpn_ip", None, Some("10.8.0.2"))
            .await
            .unwrap();
        sqlx::query("DELETE FROM peers WHERE id = 'p1'")
            .execute(&pool)
            .await
            .unwrap();
        let all = repo.list(None, 50).await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].device_name.is_none());
        assert_eq!(all[0].username.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn prune_removes_old_rows() {
        let pool = setup_pool().await;
        let repo = SqlitePeerEventRepository::new(pool.clone());
        repo.insert("p1", "user-1", "endpoint", None, Some("x"))
            .await
            .unwrap();
        // created_at 是当前时刻 → cutoff 过去不删、未来全删。
        assert_eq!(repo.prune_before(1000).await.unwrap(), 0);
        let future = Utc::now().timestamp_millis() + 60_000;
        assert_eq!(repo.prune_before(future).await.unwrap(), 1);
        assert!(repo.list(None, 50).await.unwrap().is_empty());
    }
}
