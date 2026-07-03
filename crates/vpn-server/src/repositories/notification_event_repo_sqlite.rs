//! SQLite repository for notification history and dedupe checks.

use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct NotificationEventRow {
    pub id: String,
    pub event_type: String,
    pub channel: String,
    pub target: String,
    pub status: String,
    pub subject: String,
    pub error: Option<String>,
    pub metadata: Option<String>,
    pub dedupe_key: String,
    pub sent_at: Option<i64>,
    pub created_at: i64,
}

type NotificationEventTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<i64>,
    i64,
);

impl From<NotificationEventTuple> for NotificationEventRow {
    fn from(r: NotificationEventTuple) -> Self {
        Self {
            id: r.0,
            event_type: r.1,
            channel: r.2,
            target: r.3,
            status: r.4,
            subject: r.5,
            error: r.6,
            metadata: r.7,
            dedupe_key: r.8,
            sent_at: r.9,
            created_at: r.10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteNotificationEventRepository {
    pool: SqlitePool,
}

impl SqliteNotificationEventRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        id: &str,
        event_type: &str,
        channel: &str,
        target: &str,
        status: &str,
        subject: &str,
        error: Option<&str>,
        metadata: Option<&str>,
        dedupe_key: &str,
        sent_at: Option<i64>,
        created_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO notification_events
               (id, event_type, channel, target, status, subject, error, metadata, dedupe_key, sent_at, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
        )
        .bind(id)
        .bind(event_type)
        .bind(channel)
        .bind(target)
        .bind(status)
        .bind(subject)
        .bind(error)
        .bind(metadata)
        .bind(dedupe_key)
        .bind(sent_at)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    pub async fn latest_after(
        &self,
        dedupe_key: &str,
        after_ms: i64,
    ) -> Result<Option<NotificationEventRow>> {
        let row: Option<NotificationEventTuple> = sqlx::query_as(
            r#"SELECT id, event_type, channel, target, status, subject, error, metadata, dedupe_key, sent_at, created_at
               FROM notification_events
               WHERE dedupe_key = ?1 AND created_at >= ?2 AND status IN ('sent', 'skipped')
               ORDER BY created_at DESC
               LIMIT 1"#,
        )
        .bind(dedupe_key)
        .bind(after_ms)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(NotificationEventRow::from))
    }

    pub async fn list(
        &self,
        event_type: Option<&str>,
        status: Option<&str>,
        limit: u32,
    ) -> Result<Vec<NotificationEventRow>> {
        let mut sql = String::from(
            r#"SELECT id, event_type, channel, target, status, subject, error, metadata, dedupe_key, sent_at, created_at
               FROM notification_events
               WHERE 1 = 1"#,
        );
        if event_type.is_some() {
            sql.push_str(" AND event_type = ?1");
        }
        if status.is_some() {
            sql.push_str(if event_type.is_some() {
                " AND status = ?2"
            } else {
                " AND status = ?1"
            });
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");

        let mut query = sqlx::query_as::<_, NotificationEventTuple>(&sql);
        if let Some(v) = event_type {
            query = query.bind(v);
        }
        if let Some(v) = status {
            query = query.bind(v);
        }
        let rows = query
            .bind(limit.min(200) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(NotificationEventRow::from).collect())
    }
}
