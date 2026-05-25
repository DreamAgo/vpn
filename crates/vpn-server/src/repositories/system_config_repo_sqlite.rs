//! SQLite 实现的 system_config KV 仓储（Story 4.1：持久化服务端 WG 密钥等）。

use chrono::Utc;
use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct SqliteSystemConfigRepository {
    pool: SqlitePool,
}

impl SqliteSystemConfigRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 读取某 key 的值（不存在返回 None）。
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM system_config WHERE key = ?1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(|r| r.0))
    }

    /// 写入（insert or update）某 key 的值。
    pub async fn set(&self, key: &str, value: &str) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            r#"INSERT INTO system_config (key, value, updated_at)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at"#,
        )
        .bind(key)
        .bind(value)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:sysconfig_repo_test_{}?mode=memory&cache=private",
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

    #[tokio::test]
    async fn get_missing_returns_none() {
        let repo = SqliteSystemConfigRepository::new(setup_pool().await);
        assert!(repo.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let repo = SqliteSystemConfigRepository::new(setup_pool().await);
        repo.set("k", "v1").await.unwrap();
        assert_eq!(repo.get("k").await.unwrap().unwrap(), "v1");
    }

    #[tokio::test]
    async fn set_overwrites_existing_value() {
        let repo = SqliteSystemConfigRepository::new(setup_pool().await);
        repo.set("k", "v1").await.unwrap();
        repo.set("k", "v2").await.unwrap();
        assert_eq!(repo.get("k").await.unwrap().unwrap(), "v2");
    }
}
