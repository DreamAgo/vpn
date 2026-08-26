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

    /// 底层连接池（供同库的其他仓储复用，避免到处传 pool）。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
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

    /// 仅在 key 不存在时写入，供环境变量的一次性种子初始化使用。
    ///
    /// 返回 true 表示本次插入成功；并发启动时只有一个进程会成功。
    pub async fn set_if_absent(&self, key: &str, value: &str) -> Result<bool> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query(
            r#"INSERT INTO system_config (key, value, updated_at)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(key) DO NOTHING"#,
        )
        .bind(key)
        .bind(value)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// 在同一事务中写入两个配置项，供整组网络配置与热更新路由原子保存。
    pub async fn set_pair(&self, first: (&str, &str), second: (&str, &str)) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        for (key, value) in [first, second] {
            sqlx::query(
                r#"INSERT INTO system_config (key, value, updated_at)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at"#,
            )
            .bind(key)
            .bind(value)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        }
        tx.commit()
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

    #[tokio::test]
    async fn set_if_absent_never_overwrites_existing_value() {
        let repo = SqliteSystemConfigRepository::new(setup_pool().await);
        assert!(repo.set_if_absent("k", "seed").await.unwrap());
        assert!(!repo.set_if_absent("k", "changed").await.unwrap());
        assert_eq!(repo.get("k").await.unwrap().as_deref(), Some("seed"));
    }
}
