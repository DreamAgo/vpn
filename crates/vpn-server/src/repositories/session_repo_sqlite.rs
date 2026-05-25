//! SQLite 实现的 SessionRepository（Refresh Token 持久化与撤销）。

use chrono::Utc;
use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub refresh_token_hash: String,
    pub ip_addr: Option<String>,
    pub user_agent: Option<String>,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

/// 数据库行元组（与 SELECT 列顺序一致），仅用于内部反序列化。
type SessionRowTuple = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    i64,
);

impl From<SessionRowTuple> for SessionRow {
    fn from(r: SessionRowTuple) -> Self {
        SessionRow {
            id: r.0,
            user_id: r.1,
            refresh_token_hash: r.2,
            ip_addr: r.3,
            user_agent: r.4,
            expires_at: r.5,
            revoked_at: r.6,
            created_at: r.7,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteSessionRepository {
    pool: SqlitePool,
}

impl SqliteSessionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        id: &str,
        user_id: &str,
        refresh_token_hash: &str,
        ip_addr: Option<&str>,
        user_agent: Option<&str>,
        expires_at_ms: i64,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            r#"INSERT INTO sessions (id, user_id, refresh_token_hash, ip_addr, user_agent, expires_at, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        )
        .bind(id)
        .bind(user_id)
        .bind(refresh_token_hash)
        .bind(ip_addr)
        .bind(user_agent)
        .bind(expires_at_ms)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    /// 查找未撤销且未过期的 session。
    pub async fn find_active_by_token_hash(
        &self,
        hash: &str,
        now_ms: i64,
    ) -> Result<Option<SessionRow>> {
        let row: Option<SessionRowTuple> = sqlx::query_as(
            r#"SELECT id, user_id, refresh_token_hash, ip_addr, user_agent, expires_at, revoked_at, created_at
                   FROM sessions
                   WHERE refresh_token_hash = ?1 AND revoked_at IS NULL AND expires_at > ?2"#,
        )
        .bind(hash)
        .bind(now_ms)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(SessionRow::from))
    }

    /// 撤销单个 session。
    pub async fn revoke(&self, hash: &str) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query("UPDATE sessions SET revoked_at = ?1 WHERE refresh_token_hash = ?2 AND revoked_at IS NULL")
            .bind(now)
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    /// 撤销某用户的所有活跃 session（用于修改密码、删除用户等场景）。
    pub async fn revoke_all_for_user(&self, user_id: &str) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    async fn setup_pool() -> SqlitePool {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;
        let url = format!(
            "sqlite:file:session_repo_test_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        // 需要先插一个 user，否则 sessions 表 FK（虽 SQLite 不强制但语义如此）
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-1', 'alice', 'a@e.com', 'h', 'admin', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn create_and_find_session() {
        let pool = setup_pool().await;
        let repo = SqliteSessionRepository::new(pool);
        let now = Utc::now().timestamp_millis();
        let exp = (Utc::now() + Duration::days(30)).timestamp_millis();
        repo.create(
            "s1",
            "user-1",
            "hash-abc",
            Some("127.0.0.1"),
            Some("test-ua"),
            exp,
        )
        .await
        .unwrap();
        let found = repo
            .find_active_by_token_hash("hash-abc", now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "s1");
        assert_eq!(found.user_id, "user-1");
    }

    #[tokio::test]
    async fn revoked_session_not_found_as_active() {
        let pool = setup_pool().await;
        let repo = SqliteSessionRepository::new(pool);
        let now = Utc::now().timestamp_millis();
        let exp = (Utc::now() + Duration::days(30)).timestamp_millis();
        repo.create("s1", "user-1", "hash-abc", None, None, exp)
            .await
            .unwrap();
        repo.revoke("hash-abc").await.unwrap();
        let result = repo
            .find_active_by_token_hash("hash-abc", now)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn expired_session_not_found_as_active() {
        let pool = setup_pool().await;
        let repo = SqliteSessionRepository::new(pool);
        let now = Utc::now().timestamp_millis();
        let exp = (Utc::now() - Duration::days(1)).timestamp_millis(); // already expired
        repo.create("s1", "user-1", "hash-abc", None, None, exp)
            .await
            .unwrap();
        let result = repo
            .find_active_by_token_hash("hash-abc", now)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn revoke_all_for_user_revokes_multiple_sessions() {
        let pool = setup_pool().await;
        let repo = SqliteSessionRepository::new(pool);
        let now = Utc::now().timestamp_millis();
        let exp = (Utc::now() + Duration::days(30)).timestamp_millis();
        repo.create("s1", "user-1", "h1", None, None, exp)
            .await
            .unwrap();
        repo.create("s2", "user-1", "h2", None, None, exp)
            .await
            .unwrap();
        repo.revoke_all_for_user("user-1").await.unwrap();
        assert!(repo
            .find_active_by_token_hash("h1", now)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .find_active_by_token_hash("h2", now)
            .await
            .unwrap()
            .is_none());
    }
}
