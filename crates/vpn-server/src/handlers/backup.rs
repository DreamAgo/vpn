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

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

const BACKUP_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupArchive {
    format_version: u32,
    generated_at: i64,
    product: String,
    tables: BackupTables,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupTables {
    users: Vec<UserRow>,
    user_groups: Vec<UserGroupRow>,
    user_group_members: Vec<UserGroupMemberRow>,
    subnets: Vec<SubnetRow>,
    peers: Vec<PeerRow>,
    system_config: Vec<SystemConfigRow>,
    audit_logs: Vec<AuditLogRow>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResponse {
    restored_at: i64,
    users: usize,
    peers: usize,
    user_groups: usize,
    subnets: usize,
    audit_logs: usize,
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
    let bytes = serde_json::to_vec_pretty(&archive)
        .map_err(|e| AppError::Internal(Box::new(e)))?;
    let filename = format!("yilian-backup-{}.json", Utc::now().format("%Y%m%d-%H%M%S"));

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
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
    if archive.format_version != BACKUP_FORMAT_VERSION {
        return Err(AppError::Validation(format!(
            "不支持的备份版本: {}",
            archive.format_version
        ))
        .into());
    }
    if archive.tables.users.is_empty() {
        return Err(AppError::Validation("备份中没有用户数据".to_string()).into());
    }

    let pool = state.db_pool()?;
    restore_archive(&pool, &archive).await?;

    Ok(success(
        &state,
        RestoreResponse {
            restored_at: state.clock.now_unix_ms(),
            users: archive.tables.users.len(),
            peers: archive.tables.peers.len(),
            user_groups: archive.tables.user_groups.len(),
            subnets: archive.tables.subnets.len(),
            audit_logs: archive.tables.audit_logs.len(),
            requires_restart: true,
        },
    ))
}

async fn create_backup(pool: &SqlitePool) -> Result<BackupArchive, AppError> {
    let tables = BackupTables {
        users: sqlx::query_as::<_, UserRow>(
            "SELECT id, username, email, password_hash, role, status, must_change_password,
                    last_login_at, created_at, updated_at, group_id FROM users ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        user_groups: sqlx::query_as::<_, UserGroupRow>(
            "SELECT id, name, routes, created_at, updated_at FROM user_groups ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        user_group_members: sqlx::query_as::<_, UserGroupMemberRow>(
            "SELECT user_id, group_id FROM user_group_members ORDER BY user_id, group_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        subnets: sqlx::query_as::<_, SubnetRow>(
            "SELECT id, name, cidr, created_at, updated_at FROM subnets ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        peers: sqlx::query_as::<_, PeerRow>(
            "SELECT id, user_id, device_name, wg_public_key, vpn_ip, endpoint, os_info,
                    last_seen_at, status, created_at, updated_at, routed_subnets
               FROM peers ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        system_config: sqlx::query_as::<_, SystemConfigRow>(
            "SELECT key, value, updated_at FROM system_config ORDER BY key",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
        audit_logs: sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, user_id, username, action, resource, ip_addr, user_agent,
                    metadata, status_code, created_at
               FROM audit_logs ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?,
    };

    Ok(BackupArchive {
        format_version: BACKUP_FORMAT_VERSION,
        generated_at: Utc::now().timestamp_millis(),
        product: "易链".to_string(),
        tables,
    })
}

async fn restore_archive(pool: &SqlitePool, archive: &BackupArchive) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    for table in [
        "audit_logs",
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

    insert_users(&mut tx, &archive.tables.users).await?;
    insert_user_groups(&mut tx, &archive.tables.user_groups).await?;
    insert_user_group_members(&mut tx, &archive.tables.user_group_members).await?;
    insert_subnets(&mut tx, &archive.tables.subnets).await?;
    insert_peers(&mut tx, &archive.tables.peers).await?;
    insert_system_config(&mut tx, &archive.tables.system_config).await?;
    insert_audit_logs(&mut tx, &archive.tables.audit_logs).await?;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    Ok(())
}

async fn insert_users(tx: &mut Transaction<'_, Sqlite>, rows: &[UserRow]) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO users (id, username, email, password_hash, role, status,
                must_change_password, last_login_at, created_at, updated_at, group_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
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

async fn insert_audit_logs(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[AuditLogRow],
) -> Result<(), AppError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, username, action, resource, ip_addr,
                user_agent, metadata, status_code, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(&row.id)
        .bind(&row.user_id)
        .bind(&row.username)
        .bind(&row.action)
        .bind(&row.resource)
        .bind(&row.ip_addr)
        .bind(&row.user_agent)
        .bind(&row.metadata)
        .bind(row.status_code)
        .bind(row.created_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
    }
    Ok(())
}
