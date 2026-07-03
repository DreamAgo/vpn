//! SQLite repository for the domain event bus.

use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct DomainEventRow {
    pub id: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload: String,
    pub status: String,
    pub created_at: i64,
    pub processed_at: Option<i64>,
}

type DomainEventTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
);

impl From<DomainEventTuple> for DomainEventRow {
    fn from(r: DomainEventTuple) -> Self {
        Self {
            id: r.0,
            event_type: r.1,
            aggregate_type: r.2,
            aggregate_id: r.3,
            payload: r.4,
            status: r.5,
            created_at: r.6,
            processed_at: r.7,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteDomainEventRepository {
    pool: SqlitePool,
}

impl SqliteDomainEventRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        id: &str,
        event_type: &str,
        aggregate_type: &str,
        aggregate_id: &str,
        payload: &str,
        created_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO domain_events
               (id, event_type, aggregate_type, aggregate_id, payload, status, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)"#,
        )
        .bind(id)
        .bind(event_type)
        .bind(aggregate_type)
        .bind(aggregate_id)
        .bind(payload)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    pub async fn list_pending(&self, limit: u32) -> Result<Vec<DomainEventRow>> {
        let rows: Vec<DomainEventTuple> = sqlx::query_as(
            r#"SELECT id, event_type, aggregate_type, aggregate_id, payload, status, created_at, processed_at
               FROM domain_events
               WHERE status = 'pending'
               ORDER BY created_at ASC
               LIMIT ?1"#,
        )
        .bind(limit.min(500) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(DomainEventRow::from).collect())
    }

    pub async fn mark_processed(&self, id: &str, processed_at: i64) -> Result<u64> {
        let res = sqlx::query(
            r#"UPDATE domain_events
               SET status = 'processed', processed_at = ?1
               WHERE id = ?2 AND status = 'pending'"#,
        )
        .bind(processed_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(res.rows_affected())
    }
}
