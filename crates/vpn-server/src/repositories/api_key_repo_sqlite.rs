//! SQLite repository for service account API keys.

use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub scopes: String,
    pub status: String,
    pub created_by: String,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

type ApiKeyRowTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    i64,
);

impl From<ApiKeyRowTuple> for ApiKeyRow {
    fn from(r: ApiKeyRowTuple) -> Self {
        Self {
            id: r.0,
            name: r.1,
            key_hash: r.2,
            scopes: r.3,
            status: r.4,
            created_by: r.5,
            last_used_at: r.6,
            revoked_at: r.7,
            created_at: r.8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteApiKeyRepository {
    pool: SqlitePool,
}

impl SqliteApiKeyRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        id: &str,
        name: &str,
        key_hash: &str,
        scopes: &str,
        created_by: &str,
        now_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO api_keys (id, name, key_hash, scopes, status, created_by, created_at)
               VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)"#,
        )
        .bind(id)
        .bind(name)
        .bind(key_hash)
        .bind(scopes)
        .bind(created_by)
        .bind(now_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<ApiKeyRow>> {
        let rows: Vec<ApiKeyRowTuple> = sqlx::query_as(
            r#"SELECT id, name, key_hash, scopes, status, created_by, last_used_at, revoked_at, created_at
               FROM api_keys
               ORDER BY created_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(ApiKeyRow::from).collect())
    }

    pub async fn find_active_by_hash(&self, key_hash: &str) -> Result<Option<ApiKeyRow>> {
        let row: Option<ApiKeyRowTuple> = sqlx::query_as(
            r#"SELECT id, name, key_hash, scopes, status, created_by, last_used_at, revoked_at, created_at
               FROM api_keys
               WHERE key_hash = ?1 AND status = 'active' AND revoked_at IS NULL
               LIMIT 1"#,
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(ApiKeyRow::from))
    }

    pub async fn touch_last_used(&self, id: &str, now_ms: i64) -> Result<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2")
            .bind(now_ms)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    pub async fn revoke(&self, id: &str, now_ms: i64) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE api_keys SET status = 'revoked', revoked_at = ?1 WHERE id = ?2 AND status = 'active'",
        )
        .bind(now_ms)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(res.rows_affected())
    }
}
