//! Request-scoped identity and transactional audit helpers. Never capture HTTP bodies.
use crate::repositories::AuditLogEntry;
use serde_json::{json, Value};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use vpn_core::{AppError, Result};

#[derive(Clone)]
pub struct AuditContext {
    pub entry: AuditLogEntry,
    pub request_id: String,
    pub committed: Arc<AtomicUsize>,
}
pub static FAILED_TRANSACTION_WRITES: AtomicU64 = AtomicU64::new(0);
tokio::task_local! { pub static CONTEXT: AuditContext; }
pub fn committed() {
    let _ = CONTEXT.try_with(|c| c.committed.fetch_add(1, Ordering::Relaxed));
}
fn db(e: sqlx::Error) -> AppError {
    AppError::Database(Box::new(e))
}

/// Only these explicit columns may enter audit snapshots. Secrets are compared in memory
/// and replaced with a marker before persistence; timestamps and heartbeat state are excluded.
pub async fn snapshot(conn: &mut SqliteConnection, table: &str, id: &str) -> Result<Value> {
    if CONTEXT.try_with(|_| ()).is_err() {
        return Ok(Value::Null);
    }
    let (columns, key) = match table {
        "users" => ("'username',username,'email',email,'role',role,'status',status,'max_devices',max_devices,'access_mode',access_mode,'password_hash',password_hash", "id"),
        "user_groups" => ("'name',name,'routes',routes", "id"),
        "subnets" => ("'name',name,'cidr',cidr", "id"),
        "api_keys" => ("'name',name,'scopes',scopes,'status',status", "id"),
        "peers" => ("'user_id',user_id,'device_name',device_name,'vpn_ip',vpn_ip,'status',status,'routed_subnets',routed_subnets", "id"),
        "system_config" => {
            let raw: Option<String> = sqlx::query_scalar("SELECT value FROM system_config WHERE key=?")
                .bind(id).fetch_optional(conn).await.map_err(db)?;
            return Ok(raw.map(|v| serde_json::from_str(&v).unwrap_or(Value::String(v))).unwrap_or(Value::Null));
        }
        "peers_by_user" => {
            let rows:Vec<String> = sqlx::query_scalar("SELECT json_object('id',id,'status',status,'vpn_ip',vpn_ip,'device_name',device_name) FROM peers WHERE user_id=? ORDER BY id").bind(id).fetch_all(conn).await.map_err(db)?;
            return rows.into_iter().map(|raw|serde_json::from_str(&raw).map_err(|e|AppError::Internal(Box::new(e)))).collect::<Result<Vec<Value>>>().map(Value::Array);
        }
        "user_group_members" => {
            let ids: Vec<String> = sqlx::query_scalar("SELECT group_id FROM user_group_members WHERE user_id=? ORDER BY group_id")
                .bind(id).fetch_all(conn).await.map_err(db)?;
            return Ok(json!({"group_ids": ids}));
        }
        _ => return Err(AppError::Validation("不支持的审计资源".into())),
    };
    let sql = format!("SELECT json_object({columns}) FROM {table} WHERE {key}=?");
    let raw: Option<String> = sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    raw.map(|v| serde_json::from_str(&v).map_err(|e| AppError::Internal(Box::new(e))))
        .transpose()
        .map(|v| v.unwrap_or(Value::Null))
}
fn sensitive(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "private",
        "psk",
        "key_hash",
        "encrypt_key",
        "webhook_url",
        "feishu_url",
        "dingtalk_url",
    ]
    .iter()
    .any(|s| p.contains(s))
}
fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if sensitive(k) {
                            json!({"changed":true})
                        } else {
                            redact(v)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}
fn diff(path: &str, before: &Value, after: &Value, changes: &mut serde_json::Map<String, Value>) {
    if before == after {
        return;
    }
    if sensitive(path) {
        changes.insert(path.into(), json!({"changed": true}));
        return;
    }
    if before.is_object() || after.is_object() {
        let keys: std::collections::BTreeSet<_> = before
            .as_object()
            .into_iter()
            .flat_map(|o| o.keys())
            .chain(after.as_object().into_iter().flat_map(|o| o.keys()))
            .collect();
        for key in keys {
            diff(&format!("{path}.{key}"), &before[key], &after[key], changes);
        }
    } else {
        changes.insert(
            path.into(),
            json!({"before": redact(before), "after": redact(after)}),
        );
    }
}

pub async fn record(
    conn: &mut SqliteConnection,
    target: &str,
    before: Value,
    after: Value,
) -> Result<()> {
    if before == after {
        return Ok(());
    }
    let Ok(ctx) = CONTEXT.try_with(Clone::clone) else {
        return Ok(());
    };
    let mut changes = serde_json::Map::new();
    diff(target, &before, &after, &mut changes);
    let metadata = json!({"request_id":ctx.request_id,"outcome":"committed","target":target,"changes":changes});
    let mut e = ctx.entry;
    if e.action == "network.settings.update" && changes.keys().any(|key| key.contains(".dns.")) {
        e.action = "network.dns.update".into();
    }
    sqlx::query("INSERT INTO audit_logs(id,user_id,username,action,resource,ip_addr,user_agent,metadata,status_code,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(uuid::Uuid::now_v7().to_string()).bind(e.user_id).bind(e.username).bind(&e.action)
        .bind(target).bind(e.ip_addr).bind(e.user_agent).bind(metadata.to_string()).bind(200)
        .bind(chrono::Utc::now().timestamp_millis()).execute(conn).await.map_err(|error| {
            FAILED_TRANSACTION_WRITES.fetch_add(1, Ordering::Relaxed);
            tracing::error!(action=%e.action, "业务审计写入失败，回滚事务");
            db(error)
        })?;
    Ok(())
}
pub async fn begin(
    pool: &SqlitePool,
    table: &str,
    id: &str,
) -> Result<(Transaction<'static, Sqlite>, Value)> {
    // Acquire the write reservation before reading the old value (no upgrade races).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
    let before = snapshot(&mut tx, table, id).await?;
    Ok((tx, before))
}
pub async fn finish(
    mut tx: Transaction<'_, Sqlite>,
    table: &str,
    id: &str,
    before: Value,
) -> Result<()> {
    let after = snapshot(&mut tx, table, id).await?;
    record(&mut tx, &format!("{table}/{id}"), before, after).await?;
    tx.commit().await.map_err(db)?;
    committed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diff_redacts_secrets_and_only_includes_changes() {
        let mut changes = serde_json::Map::new();
        diff(
            "settings",
            &json!({"password":"old", "dns":false,"same":1}),
            &json!({"password":"new", "dns":true,"same":1}),
            &mut changes,
        );
        assert_eq!(changes["settings.password"], json!({"changed":true}));
        assert_eq!(
            changes["settings.dns"],
            json!({"before":false,"after":true})
        );
        assert_eq!(changes.len(), 2);
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use crate::repositories::{SqliteSystemConfigRepository, SqliteUserRepository};
    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }
    fn context() -> AuditContext {
        AuditContext {
            entry: AuditLogEntry {
                user_id: Some("api_key:test".into()),
                username: Some("automation".into()),
                action: "network.settings.update".into(),
                ..Default::default()
            },
            request_id: "request-1".into(),
            committed: Arc::new(AtomicUsize::new(0)),
        }
    }
    #[tokio::test]
    async fn audit_failure_rolls_back_all_configuration_keys() {
        let pool = pool().await;
        let repo = SqliteSystemConfigRepository::new(pool.clone());
        repo.set_many(&[
            (
                "network_settings_v3",
                r#"{"settings":{"dns":{"mode":"disabled"}}}"#,
            ),
            ("server_routes", "old"),
        ])
        .await
        .unwrap();
        sqlx::query("CREATE TRIGGER reject_audit BEFORE INSERT ON audit_logs BEGIN SELECT RAISE(ABORT,'audit unavailable'); END").execute(&pool).await.unwrap();
        let ctx = context();
        let result = CONTEXT
            .scope(
                ctx.clone(),
                repo.set_many(&[
                    (
                        "network_settings_v3",
                        r#"{"settings":{"dns":{"mode":"global"}}}"#,
                    ),
                    ("server_routes", "new"),
                ]),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(ctx.committed.load(Ordering::Relaxed), 0);
        assert_eq!(
            repo.get("server_routes").await.unwrap().as_deref(),
            Some("old")
        );
        assert!(repo
            .get("network_settings_v3")
            .await
            .unwrap()
            .unwrap()
            .contains("disabled"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn actor_diff_and_secret_markers_are_durable_together() {
        let pool = pool().await;
        let repo = SqliteSystemConfigRepository::new(pool.clone());
        repo.set(
            "network_settings_v3",
            r#"{"settings":{"dns":{"mode":"disabled"}},"encrypt_key":"old"}"#,
        )
        .await
        .unwrap();
        CONTEXT
            .scope(
                context(),
                repo.set(
                    "network_settings_v3",
                    r#"{"settings":{"dns":{"mode":"global"}},"encrypt_key":"very-secret"}"#,
                ),
            )
            .await
            .unwrap();
        let row: (String, String, String, String) =
            sqlx::query_as("SELECT user_id,username,action,metadata FROM audit_logs")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (&*row.0, &*row.1, &*row.2),
            ("api_key:test", "automation", "network.dns.update")
        );
        assert!(!row.3.contains("very-secret"));
        assert!(!row.3.contains("old"));
        let meta: Value = serde_json::from_str(&row.3).unwrap();
        assert_eq!(
            meta["changes"]["system_config.network_settings_v3.settings.dns.mode"],
            json!({"before":"disabled","after":"global"})
        );
    }
    #[tokio::test]
    async fn user_password_change_cannot_commit_without_audit() {
        let pool = pool().await;
        let repo = SqliteUserRepository::new(pool.clone());
        repo.insert("u", "alice", "a@example.com", "old-hash", "user", false, 1)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER reject_audit BEFORE INSERT ON audit_logs BEGIN SELECT RAISE(ABORT,'audit unavailable'); END").execute(&pool).await.unwrap();
        assert!(CONTEXT
            .scope(context(), repo.update_password("u", "new-hash", true))
            .await
            .is_err());
        assert_eq!(
            repo.find_by_id("u").await.unwrap().unwrap().password_hash,
            "old-hash"
        );
    }
}
