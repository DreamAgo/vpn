//! 管理员备份与恢复接口。

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};
use vpn_api_types::ApiResponse;
use vpn_core::AppError;

use crate::{
    auth::RequireAdmin,
    error::ApiError,
    services::feishu_directory_service::{ContactEvent, DirectoryStateRow},
    state::AppState,
};

const BACKUP_FORMAT_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupArchive {
    format_version: u32,
    generated_at: i64,
    product: String,
    tables: BackupTables,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupTables {
    #[serde(default)]
    approval_mail_outbox: Vec<crate::repositories::access_grant_repo_sqlite::ApprovalMailRow>,
    users: Vec<UserRow>,
    user_groups: Vec<UserGroupRow>,
    user_group_members: Vec<UserGroupMemberRow>,
    subnets: Vec<SubnetRow>,
    peers: Vec<PeerRow>,
    system_config: Vec<SystemConfigRow>,
    audit_logs: Vec<AuditLogRow>,
    #[serde(default)]
    api_keys: Vec<ApiKeyRow>,
    #[serde(default)]
    external_identities: Vec<ExternalIdentityRow>,
    #[serde(default)]
    access_grants: Vec<AccessGrantRow>,
    #[serde(default)]
    feishu_approval_inbox: Vec<ApprovalInboxRow>,
    #[serde(default)]
    feishu_user_states: Vec<DirectoryStateRow>,
    #[serde(default)]
    feishu_contact_inbox: Vec<ContactEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct UserRow {
    id: String,
    username: String,
    email: String,
    password_hash: String,
    role: String,
    status: String,
    must_change_password: i64,
    last_login_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    group_id: Option<String>,
    #[serde(default = "legacy_access_mode")]
    access_mode: String,
}

fn legacy_access_mode() -> String {
    "legacy".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct ExternalIdentityRow {
    provider: String,
    subject: String,
    user_id: String,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct AccessGrantRow {
    id: String,
    approval_instance_code: String,
    user_id: String,
    group_id: String,
    expires_at: i64,
    reason: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct ApprovalInboxRow {
    event_id: String,
    instance_code: String,
    payload_hash: String,
    payload: String,
    status: String,
    attempts: i64,
    next_attempt_at: i64,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct UserGroupRow {
    id: String,
    name: String,
    routes: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct UserGroupMemberRow {
    user_id: String,
    group_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct SubnetRow {
    id: String,
    name: String,
    cidr: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct PeerRow {
    id: String,
    user_id: String,
    device_name: String,
    wg_public_key: String,
    vpn_ip: String,
    endpoint: Option<String>,
    os_info: Option<String>,
    last_seen_at: Option<i64>,
    status: String,
    created_at: i64,
    updated_at: i64,
    routed_subnets: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct SystemConfigRow {
    key: String,
    value: String,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct AuditLogRow {
    id: String,
    user_id: Option<String>,
    username: Option<String>,
    action: String,
    resource: String,
    ip_addr: Option<String>,
    user_agent: Option<String>,
    metadata: Option<String>,
    status_code: Option<i64>,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct ApiKeyRow {
    id: String,
    name: String,
    key_hash: String,
    scopes: String,
    status: String,
    created_by: String,
    last_used_at: Option<i64>,
    revoked_at: Option<i64>,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResponse {
    restored_at: i64,
    users: usize,
    peers: usize,
    user_groups: usize,
    subnets: usize,
    audit_logs: usize,
    api_keys: usize,
    requires_restart: bool,
}

fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    ))
}

pub async fn download_backup(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
) -> Result<Response, ApiError> {
    let pool = state.db_pool()?;
    let archive = create_backup(&pool).await?;
    let bytes = serde_json::to_vec_pretty(&archive).map_err(|e| AppError::Internal(Box::new(e)))?;
    let filename = format!("yilian-backup-{}.json", Utc::now().format("%Y%m%d-%H%M%S"));

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            ),
        ],
        bytes,
    )
        .into_response())
}

pub async fn restore_backup(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(archive): Json<BackupArchive>,
) -> Result<Json<ApiResponse<RestoreResponse>>, ApiError> {
    // v1 备份没有审批字段，serde default 会按 legacy/空授权安全恢复；新版导出使用 v2，
    // 让旧版服务端明确拒绝而不是忽略授权字段后把账号静默降级。
    if !matches!(archive.format_version, 1 | 2 | 3 | BACKUP_FORMAT_VERSION) {
        return Err(
            AppError::Validation(format!("不支持的备份版本: {}", archive.format_version)).into(),
        );
    }
    if archive.tables.users.is_empty() {
        return Err(AppError::Validation("备份中没有用户数据".to_string()).into());
    }

    let pool = state.db_pool()?;
    // 恢复期间暂停审批任务的认领和提交，避免旧快照与新授权交叉写入。
    let _approval_pause = match &state.feishu_approval_service {
        Some(service) => Some(service.pause_worker().await),
        None => None,
    };
    let _directory_pause = match &state.feishu_directory_service {
        Some(service) => Some(service.pause_worker().await),
        None => None,
    };
    restore_archive(&pool, &archive).await?;
    state.refresh_network_acl().await?;

    Ok(success(
        &state,
        RestoreResponse {
            restored_at: state.clock.now_unix_ms(),
            users: archive.tables.users.len(),
            peers: archive.tables.peers.len(),
            user_groups: archive.tables.user_groups.len(),
            subnets: archive.tables.subnets.len(),
            audit_logs: 0, // No audit history is imported; live history is preserved.
            api_keys: archive.tables.api_keys.len(),
            requires_restart: true,
        },
    ))
}

async fn create_backup(pool: &SqlitePool) -> Result<BackupArchive, AppError> {
    // 单个读事务固定 SQLite snapshot，避免审批 worker 并发提交时导出悬挂引用。
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(Box::new(error)))?;
    let tables = BackupTables {
        feishu_user_states: sqlx::query_as::<_, DirectoryStateRow>("SELECT * FROM feishu_user_states ORDER BY subject")
            .fetch_all(&mut *tx).await.map_err(|e|AppError::Database(Box::new(e)))?,
        feishu_contact_inbox: sqlx::query_as::<_, ContactEvent>("SELECT * FROM feishu_contact_inbox ORDER BY event_id")
            .fetch_all(&mut *tx).await.map_err(|e|AppError::Database(Box::new(e)))?,
        users: sqlx::query_as::<_, UserRow>(
            "SELECT id, username, email, password_hash, role, status, must_change_password,
                    last_login_at, created_at, updated_at, group_id, access_mode FROM users ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        user_groups: sqlx::query_as::<_, UserGroupRow>(
            "SELECT id, name, routes, created_at, updated_at FROM user_groups ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        user_group_members: sqlx::query_as::<_, UserGroupMemberRow>(
            "SELECT user_id, group_id FROM user_group_members ORDER BY user_id, group_id",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        subnets: sqlx::query_as::<_, SubnetRow>(
            "SELECT id, name, cidr, created_at, updated_at FROM subnets ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        peers: sqlx::query_as::<_, PeerRow>(
            "SELECT id, user_id, device_name, wg_public_key, vpn_ip, endpoint, os_info,
                    last_seen_at, status, created_at, updated_at, routed_subnets
               FROM peers ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        system_config: sqlx::query_as::<_, SystemConfigRow>(
            "SELECT key, value, updated_at FROM system_config ORDER BY key",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        audit_logs: sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, user_id, username, action, resource, ip_addr, user_agent,
                    metadata, status_code, created_at
               FROM audit_logs ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        api_keys: sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, key_hash, scopes, status, created_by, last_used_at, revoked_at, created_at
               FROM api_keys ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        external_identities: sqlx::query_as::<_, ExternalIdentityRow>(
            "SELECT provider, subject, user_id, created_at FROM external_identities ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        access_grants: sqlx::query_as::<_, AccessGrantRow>(
            "SELECT id, approval_instance_code, user_id, group_id, expires_at, reason, created_at, updated_at FROM access_grants ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        approval_mail_outbox: sqlx::query_as("SELECT * FROM approval_mail_outbox ORDER BY instance_code")
            .fetch_all(&mut *tx).await.map_err(|e| AppError::Database(Box::new(e)))?,
        feishu_approval_inbox: sqlx::query_as::<_, ApprovalInboxRow>(
            "SELECT event_id, instance_code, payload_hash, payload, status, attempts, next_attempt_at, last_error, created_at, updated_at FROM feishu_approval_inbox ORDER BY created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
    };
    tx.commit()
        .await
        .map_err(|error| AppError::Database(Box::new(error)))?;

    Ok(BackupArchive {
        format_version: BACKUP_FORMAT_VERSION,
        generated_at: Utc::now().timestamp_millis(),
        product: "易链".to_string(),
        tables,
    })
}

async fn restore_archive(pool: &SqlitePool, archive: &BackupArchive) -> Result<(), AppError> {
    use sha2::Digest;
    let backup_id = format!(
        "{:x}",
        sha2::Sha256::digest(
            serde_json::to_vec(archive).map_err(|e| AppError::Internal(Box::new(e)))?
        )
    );
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    for table in [
        "approval_mail_outbox",
        "feishu_approval_inbox",
        "feishu_contact_inbox",
        "feishu_user_states",
        "access_grants",
        "external_identities",
        "api_keys",
        "sessions",
        "user_group_members",
        "subnets",
        "peers",
        "user_groups",
        "users",
        "system_config",
    ] {
        let sql = format!("DELETE FROM {table}");
        sqlx::query(&sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
    }

    for row in &archive.tables.approval_mail_outbox {
        sqlx::query("INSERT INTO approval_mail_outbox(instance_code,recipient,body,expires_at,done,attempts,next_attempt_at,last_error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")
            .bind(&row.instance_code).bind(&row.recipient).bind(&row.body).bind(row.expires_at)
            .bind(row.done).bind(row.attempts).bind(row.next_attempt_at).bind(&row.last_error)
            .execute(&mut *tx).await.map_err(|e| AppError::Database(Box::new(e)))?;
    }
    insert_users(&mut tx, &archive.tables.users).await?;
    insert_user_groups(&mut tx, &archive.tables.user_groups).await?;
    insert_external_identities(&mut tx, &archive.tables.external_identities).await?;
    insert_user_group_members(&mut tx, &archive.tables.user_group_members).await?;
    insert_access_grants(&mut tx, &archive.tables.access_grants).await?;
    insert_approval_inbox(&mut tx, &archive.tables.feishu_approval_inbox).await?;
    for row in &archive.tables.feishu_user_states {
        sqlx::query("INSERT INTO feishu_user_states(subject,app_id,open_id,directory_user_id,name,email,status,blocked,synced_at,attempted_at,last_error,last_event_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")
            .bind(&row.subject).bind(&row.app_id).bind(&row.open_id).bind(&row.directory_user_id).bind(&row.name).bind(&row.email)
            .bind(&row.status).bind(row.blocked).bind(row.synced_at).bind(row.attempted_at).bind(&row.last_error).bind(row.last_event_at)
            .execute(&mut *tx).await.map_err(|e|AppError::Database(Box::new(e)))?;
    }
    for row in &archive.tables.feishu_contact_inbox {
        sqlx::query("INSERT INTO feishu_contact_inbox(event_id,app_id,user_id,id_type,event_type,event_at,payload_hash,done,attempted_at,last_error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")
            .bind(&row.event_id).bind(&row.app_id).bind(&row.user_id).bind(&row.id_type).bind(&row.event_type).bind(row.event_at).bind(&row.payload_hash)
            .bind(row.done).bind(row.attempted_at).bind(&row.last_error).execute(&mut *tx).await.map_err(|e|AppError::Database(Box::new(e)))?;
    }
    insert_subnets(&mut tx, &archive.tables.subnets).await?;
    insert_peers(&mut tx, &archive.tables.peers).await?;
    insert_system_config(&mut tx, &archive.tables.system_config).await?;
    crate::middleware::audit_context::record(&mut tx, "backup/restore", serde_json::Value::Null,
        serde_json::json!({"backup_id":backup_id,"generated_at":archive.generated_at,"format_version":archive.format_version,
            "users":archive.tables.users.len(),"peers":archive.tables.peers.len(),"audit_history_preserved":true})).await?;
    insert_api_keys(&mut tx, &archive.tables.api_keys).await?;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    crate::middleware::audit_context::committed();
    Ok(())
}

async fn insert_users(tx: &mut Transaction<'_, Sqlite>, rows: &[UserRow]) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO users (id, username, email, password_hash, role, status,
                must_change_password, last_login_at, created_at, updated_at, group_id, access_mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(&row.id)
        .bind(&row.username)
        .bind(&row.email)
        .bind(&row.password_hash)
        .bind(&row.role)
        .bind(&row.status)
        .bind(row.must_change_password)
        .bind(row.last_login_at)
        .bind(row.created_at)
        .bind(row.updated_at)
        .bind(&row.group_id)
        .bind(&row.access_mode)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_external_identities(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[ExternalIdentityRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO external_identities(provider,subject,user_id,created_at) VALUES(?1,?2,?3,?4)",
        )
        .bind(&row.provider)
        .bind(&row.subject)
        .bind(&row.user_id)
        .bind(row.created_at)
        .execute(&mut **tx)
        .await
        .map_err(|error| AppError::Database(Box::new(error)))?;
    }
    Ok(())
}

async fn insert_access_grants(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[AccessGrantRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            r#"INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,reason,created_at,updated_at)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,
        )
        .bind(&row.id)
        .bind(&row.approval_instance_code)
        .bind(&row.user_id)
        .bind(&row.group_id)
        .bind(row.expires_at)
        .bind(&row.reason)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(&mut **tx)
        .await
        .map_err(|error| AppError::Database(Box::new(error)))?;
    }
    Ok(())
}

async fn insert_approval_inbox(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[ApprovalInboxRow],
) -> Result<(), AppError> {
    let now = Utc::now().timestamp_millis();
    for row in rows {
        // 备份发生时正在执行的任务在恢复后必须可重新认领。
        let status = if row.status == "processing" {
            "retry"
        } else {
            row.status.as_str()
        };
        sqlx::query(
            r#"INSERT INTO feishu_approval_inbox(event_id,instance_code,payload_hash,payload,status,attempts,next_attempt_at,last_error,created_at,updated_at)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"#,
        )
        .bind(&row.event_id)
        .bind(&row.instance_code)
        .bind(&row.payload_hash)
        .bind(&row.payload)
        .bind(status)
        .bind(row.attempts)
        .bind(if status == "retry" { now } else { row.next_attempt_at })
        .bind(&row.last_error)
        .bind(row.created_at)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|error| AppError::Database(Box::new(error)))?;
    }
    Ok(())
}

async fn insert_user_groups(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[UserGroupRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO user_groups (id, name, routes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.routes)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_user_group_members(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[UserGroupMemberRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query("INSERT INTO user_group_members (user_id, group_id) VALUES (?1, ?2)")
            .bind(&row.user_id)
            .bind(&row.group_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_subnets(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[SubnetRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO subnets (id, name, cidr, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.cidr)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_peers(tx: &mut Transaction<'_, Sqlite>, rows: &[PeerRow]) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO peers (id, user_id, device_name, wg_public_key, vpn_ip, endpoint,
                os_info, last_seen_at, status, created_at, updated_at, routed_subnets)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(&row.id)
        .bind(&row.user_id)
        .bind(&row.device_name)
        .bind(&row.wg_public_key)
        .bind(&row.vpn_ip)
        .bind(&row.endpoint)
        .bind(&row.os_info)
        .bind(row.last_seen_at)
        .bind(&row.status)
        .bind(row.created_at)
        .bind(row.updated_at)
        .bind(&row.routed_subnets)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_system_config(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[SystemConfigRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query("INSERT INTO system_config (key, value, updated_at) VALUES (?1, ?2, ?3)")
            .bind(&row.key)
            .bind(&row.value)
            .bind(row.updated_at)
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

async fn insert_api_keys(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[ApiKeyRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, scopes, status, created_by,
                last_used_at, revoked_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.key_hash)
        .bind(&row.scopes)
        .bind(&row.status)
        .bind(&row.created_by)
        .bind(row.last_used_at)
        .bind(row.revoked_at)
        .bind(row.created_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}

#[cfg(test)]
mod directory_backup_tests {
    use super::*;
    #[tokio::test]
    async fn backup_roundtrip_preserves_independent_feishu_restrictions_and_events() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO users(id,username,email,password_hash,role,status,must_change_password,created_at,updated_at) VALUES('u1','alice','a@example.com','hash','user','active',0,0,0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO external_identities VALUES('feishu','union1','u1',0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO feishu_user_states(subject,app_id,status,blocked,last_event_at) VALUES('union1','app','frozen',1,100)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO feishu_contact_inbox(event_id,app_id,user_id,id_type,event_type,event_at,payload_hash) VALUES('e1','app','union1','union_id','contact.user.updated_v3',100,'hash')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO approval_mail_outbox(instance_code,recipient,body,expires_at,done,attempts,next_attempt_at) VALUES('mail1','a@example.com','approved',999999,1,2,100)").execute(&pool).await.unwrap();
        let archive = create_backup(&pool).await.unwrap();
        assert_eq!(archive.format_version, 4);
        let encoded = serde_json::to_vec(&archive).unwrap();
        let archive: BackupArchive = serde_json::from_slice(&encoded).unwrap();
        sqlx::query("INSERT INTO audit_logs(id,action,resource,created_at) VALUES('live-history','user_update','users/u1',1)").execute(&pool).await.unwrap();
        let mut archive = archive;
        archive.tables.audit_logs.push(AuditLogRow {
            id: "forged-history".into(),
            user_id: None,
            username: None,
            action: "forged".into(),
            resource: "users/u1".into(),
            ip_addr: None,
            user_agent: None,
            metadata: None,
            status_code: None,
            created_at: 0,
        });
        restore_archive(&pool, &archive).await.unwrap();
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM audit_logs")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec!["live-history"]);

        let user = crate::repositories::SqliteUserRepository::new(pool.clone());
        assert_eq!(
            user.find_by_id("u1").await.unwrap().unwrap().status,
            "active"
        );
        assert!(matches!(
            user.ensure_available("u1").await,
            Err(AppError::AccountDisabled)
        ));
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM feishu_contact_inbox WHERE done=0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        let mail: (bool, i64) = sqlx::query_as(
            "SELECT done,attempts FROM approval_mail_outbox WHERE instance_code='mail1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(mail, (true, 2));
    }
}
