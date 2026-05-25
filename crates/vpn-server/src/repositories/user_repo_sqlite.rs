//! SQLite 实现的 UserRepository。

use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

/// 用户列表查询过滤条件（已归一化：page/page_size 已套用默认值，order_by 已白名单校验）。
#[derive(Debug, Clone)]
pub struct UserListFilter {
    /// 模糊匹配 username / email（None 表示不过滤）
    pub search: Option<String>,
    /// 状态筛选 "active" | "disabled"（None 表示不过滤）
    pub status: Option<String>,
    /// 排序列（已白名单校验，可安全拼接进 SQL）
    pub order_by: OrderByColumn,
    /// 1-based 页码
    pub page: u32,
    /// 每页条数
    pub page_size: u32,
}

/// 允许的排序列（白名单，防 SQL 注入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderByColumn {
    CreatedAt,
    Username,
    LastLoginAt,
}

impl OrderByColumn {
    /// 从用户输入解析，非法值回退 CreatedAt。
    pub fn parse(input: Option<&str>) -> Self {
        match input {
            Some("username") => OrderByColumn::Username,
            Some("last_login_at") => OrderByColumn::LastLoginAt,
            _ => OrderByColumn::CreatedAt,
        }
    }

    /// 对应的 SQL ORDER BY 子句（含方向；仅由白名单常量构成）。
    fn sql_clause(self) -> &'static str {
        match self {
            OrderByColumn::CreatedAt => "created_at DESC",
            OrderByColumn::Username => "username ASC",
            OrderByColumn::LastLoginAt => "last_login_at DESC",
        }
    }
}

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

    /// 更新用户状态（"active" | "disabled"）。返回受影响行数（0 表示用户不存在）。
    pub async fn update_status(&self, id: &str, status: &str) -> Result<u64> {
        let now = Utc::now().timestamp_millis();
        let result = sqlx::query("UPDATE users SET status = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(status)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(result.rows_affected())
    }

    /// 在单事务内删除用户及其所有 session（级联）。返回删除的 user 行数。
    ///
    /// 任一步失败则回滚。
    // TODO(Epic 4): 级联删除该用户 peers + WireGuard runtime 清理
    pub async fn delete_with_sessions(&self, id: &str) -> Result<u64> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;

        sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;

        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;

        tx.commit()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;

        Ok(result.rows_affected())
    }

    /// 统计符合过滤条件的用户总数（用于分页 total）。
    pub async fn count(&self, filter: &UserListFilter) -> Result<i64> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) FROM users");
        Self::push_where(&mut qb, filter);
        let count: (i64,) = qb
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(count.0)
    }

    /// 列表查询（带搜索 / 状态筛选 / 排序 / 分页）。
    pub async fn list(&self, filter: &UserListFilter) -> Result<Vec<UserRow>> {
        let page = filter.page.max(1);
        let page_size = filter.page_size.max(1);
        let offset = (page - 1) as i64 * page_size as i64;

        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            r#"SELECT id, username, email, password_hash, role, status, must_change_password,
                      last_login_at, created_at, updated_at
               FROM users"#,
        );
        Self::push_where(&mut qb, filter);
        // order_by 由白名单枚举生成，无注入风险。
        qb.push(" ORDER BY ");
        qb.push(filter.order_by.sql_clause());
        qb.push(" LIMIT ");
        qb.push_bind(page_size as i64);
        qb.push(" OFFSET ");
        qb.push_bind(offset);

        let rows: Vec<UserRowTuple> = qb
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(UserRow::from).collect())
    }

    /// 拼接 WHERE 子句（search / status）；绑定值用占位符防注入。
    fn push_where(qb: &mut QueryBuilder<Sqlite>, filter: &UserListFilter) {
        let mut first = true;
        let mut clause = |qb: &mut QueryBuilder<Sqlite>| {
            if first {
                qb.push(" WHERE ");
                first = false;
            } else {
                qb.push(" AND ");
            }
        };

        if let Some(search) = filter.search.as_deref().filter(|s| !s.is_empty()) {
            clause(qb);
            let pattern = format!("%{}%", search);
            qb.push("(username LIKE ");
            qb.push_bind(pattern.clone());
            qb.push(" OR email LIKE ");
            qb.push_bind(pattern);
            qb.push(")");
        }

        if let Some(status) = filter.status.as_deref().filter(|s| !s.is_empty()) {
            clause(qb);
            qb.push("status = ");
            qb.push_bind(status.to_string());
        }
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

    fn filter_default() -> UserListFilter {
        UserListFilter {
            search: None,
            status: None,
            order_by: OrderByColumn::CreatedAt,
            page: 1,
            page_size: 20,
        }
    }

    #[test]
    fn order_by_parse_whitelist() {
        assert_eq!(OrderByColumn::parse(None), OrderByColumn::CreatedAt);
        assert_eq!(
            OrderByColumn::parse(Some("created_at")),
            OrderByColumn::CreatedAt
        );
        assert_eq!(
            OrderByColumn::parse(Some("username")),
            OrderByColumn::Username
        );
        assert_eq!(
            OrderByColumn::parse(Some("last_login_at")),
            OrderByColumn::LastLoginAt
        );
        // 非法输入（含注入尝试）回退 created_at
        assert_eq!(
            OrderByColumn::parse(Some("id; DROP TABLE users")),
            OrderByColumn::CreatedAt
        );
    }

    #[tokio::test]
    async fn update_status_disables_user() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        repo.insert("u1", "alice", "a@e.com", "h", "user", false)
            .await
            .unwrap();
        let affected = repo.update_status("u1", "disabled").await.unwrap();
        assert_eq!(affected, 1);
        let user = repo.find_by_id("u1").await.unwrap().unwrap();
        assert_eq!(user.status, "disabled");
    }

    #[tokio::test]
    async fn update_status_unknown_user_affects_zero_rows() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        let affected = repo.update_status("missing", "disabled").await.unwrap();
        assert_eq!(affected, 0);
    }

    #[tokio::test]
    async fn delete_with_sessions_removes_user_and_sessions() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool.clone());
        repo.insert("u1", "alice", "a@e.com", "h", "user", false)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO sessions (id, user_id, refresh_token_hash, expires_at, created_at)
               VALUES ('s1', 'u1', 'h1', 9999999999999, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let affected = repo.delete_with_sessions("u1").await.unwrap();
        assert_eq!(affected, 1);
        assert!(repo.find_by_id("u1").await.unwrap().is_none());
        let remaining: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE user_id = 'u1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }

    #[tokio::test]
    async fn delete_unknown_user_affects_zero_rows() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        let affected = repo.delete_with_sessions("missing").await.unwrap();
        assert_eq!(affected, 0);
    }

    #[tokio::test]
    async fn list_and_count_with_search_and_status() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        repo.insert("u1", "alice", "alice@example.com", "h", "user", false)
            .await
            .unwrap();
        repo.insert("u2", "bob", "bob@example.com", "h", "user", false)
            .await
            .unwrap();
        repo.insert("u3", "carol", "carol@other.com", "h", "user", false)
            .await
            .unwrap();
        repo.update_status("u3", "disabled").await.unwrap();

        // 无过滤：全部 3 个
        assert_eq!(repo.count(&filter_default()).await.unwrap(), 3);
        assert_eq!(repo.list(&filter_default()).await.unwrap().len(), 3);

        // 搜索 username
        let mut f = filter_default();
        f.search = Some("ali".to_string());
        assert_eq!(repo.count(&f).await.unwrap(), 1);
        assert_eq!(repo.list(&f).await.unwrap()[0].username, "alice");

        // 搜索 email
        let mut f = filter_default();
        f.search = Some("example.com".to_string());
        assert_eq!(repo.count(&f).await.unwrap(), 2);

        // 状态筛选
        let mut f = filter_default();
        f.status = Some("disabled".to_string());
        let disabled = repo.list(&f).await.unwrap();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].username, "carol");
    }

    #[tokio::test]
    async fn list_pagination_and_order_by_username() {
        let pool = setup_pool().await;
        let repo = SqliteUserRepository::new(pool);
        for (i, name) in ["delta", "bravo", "alpha", "charlie"].iter().enumerate() {
            repo.insert(
                &format!("u{i}"),
                name,
                &format!("{name}@e.com"),
                "h",
                "user",
                false,
            )
            .await
            .unwrap();
        }

        let mut f = filter_default();
        f.order_by = OrderByColumn::Username;
        f.page = 1;
        f.page_size = 2;
        let page1 = repo.list(&f).await.unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].username, "alpha");
        assert_eq!(page1[1].username, "bravo");

        f.page = 2;
        let page2 = repo.list(&f).await.unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].username, "charlie");
        assert_eq!(page2[1].username, "delta");
    }
}
