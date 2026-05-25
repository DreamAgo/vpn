//! SQLite 实现的 UserRepository。

use chrono::Utc;
use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

/// 数据库行元组（与 SELECT 列顺序一致），仅用于内部反序列化。
type UserRowTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    i64,
    i64,
);

impl From<UserRowTuple> for UserRow {
    fn from(r: UserRowTuple) -> Self {
        UserRow {
            id: r.0,
            username: r.1,
            email: r.2,
            password_hash: r.3,
            role: r.4,
            status: r.5,
            must_change_password: r.6 != 0,
            last_login_at: r.7,
            created_at: r.8,
            updated_at: r.9,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub status: String,
    pub must_change_password: bool,
    pub last_login_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct SqliteUserRepository {
    pool: SqlitePool,
}

impl SqliteUserRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn count_admins(&self) -> Result<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(count.0)
    }

    pub async fn find_by_username(&self, username: &str) -> Result<Option<UserRow>> {
        let row: Option<UserRowTuple> = sqlx::query_as(
            r#"SELECT id, username, email, password_hash, role, status, must_change_password,
                          last_login_at, created_at, updated_at
                   FROM users WHERE username = ?1"#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(UserRow::from))
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<UserRow>> {
        let row: Option<UserRowTuple> = sqlx::query_as(
            r#"SELECT id, username, email, password_hash, role, status, must_change_password,
                          last_login_at, created_at, updated_at
                   FROM users WHERE id = ?1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(UserRow::from))
    }

    /// 插入新用户。username/email 冲突返回 DuplicateResource。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        id: &str,
        username: &str,
        email: &str,
        password_hash: &str,
        role: &str,
        must_change_password: bool,
    ) -> Result<UserRow> {
        let now = Utc::now().timestamp_millis();
        let must_change = if must_change_password { 1 } else { 0 };
        let result = sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7)"#,
        )
        .bind(id)
        .bind(username)
        .bind(email)
        .bind(password_hash)
        .bind(role)
        .bind(must_change)
        .bind(now)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(UserRow {
                id: id.to_string(),
                username: username.to_string(),
                email: email.to_string(),
                password_hash: password_hash.to_string(),
                role: role.to_string(),
                status: "active".to_string(),
                must_change_password,
                last_login_at: None,
                created_at: now,
                updated_at: now,
            }),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(AppError::DuplicateResource("用户名或邮箱".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    pub async fn update_password(
        &self,
        id: &str,
        new_hash: &str,
        clear_must_change: bool,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let must_change = if clear_must_change { 0 } else { 1 };
        sqlx::query(
            r#"UPDATE users SET password_hash = ?1, must_change_password = ?2, updated_at = ?3 WHERE id = ?4"#,
        )
        .bind(new_hash)
        .bind(must_change)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query("UPDATE users SET last_login_at = ?1, updated_at = ?1 WHERE id = ?2")
            .bind(now)
            .bind(id)
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
        // 用唯一内存数据库（per-test 共享名 + cache=private 保证隔离）
        let url = format!(
            "sqlite:file:user_repo_test_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1) // in-memory DB 必须单连接
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn count_admins_starts_at_zero() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        assert_eq!(repo.count_admins().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn insert_and_find_by_username() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        let user = repo
            .insert(
                "u1",
                "alice",
                "alice@example.com",
                "$argon2id$dummy",
                "admin",
                false,
            )
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(repo.count_admins().await.unwrap(), 1);

        let found = repo.find_by_username("alice").await.unwrap().unwrap();
        assert_eq!(found.id, "u1");
        assert_eq!(found.role, "admin");
        assert!(!found.must_change_password);
    }

    #[tokio::test]
    async fn duplicate_username_returns_error() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        repo.insert("u1", "alice", "a@e.com", "h", "admin", false)
            .await
            .unwrap();
        let err = repo
            .insert("u2", "alice", "b@e.com", "h", "user", false)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn update_password_clears_must_change_flag() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        repo.insert("u1", "alice", "a@e.com", "old", "user", true)
            .await
            .unwrap();
        let user = repo.find_by_id("u1").await.unwrap().unwrap();
        assert!(user.must_change_password);

        repo.update_password("u1", "new", true).await.unwrap();
        let user = repo.find_by_id("u1").await.unwrap().unwrap();
        assert_eq!(user.password_hash, "new");
        assert!(!user.must_change_password);
    }
}
