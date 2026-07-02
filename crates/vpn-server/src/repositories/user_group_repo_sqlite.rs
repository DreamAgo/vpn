//! SQLite 实现的 UserGroupRepository(用户组 + 组级可路由网段)。

use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

/// 用户组数据库行。`routes` 为逗号分隔的 CIDR 字符串。
#[derive(Debug, Clone)]
pub struct UserGroupRow {
    pub id: String,
    pub name: String,
    pub routes: String,
    pub created_at: i64,
    pub updated_at: i64,
}

type GroupRowTuple = (String, String, String, i64, i64);

impl From<GroupRowTuple> for UserGroupRow {
    fn from(r: GroupRowTuple) -> Self {
        UserGroupRow {
            id: r.0,
            name: r.1,
            routes: r.2,
            created_at: r.3,
            updated_at: r.4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteUserGroupRepository {
    pool: SqlitePool,
}

impl SqliteUserGroupRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 列出所有组 + 各组成员数(按名称排序)。
    pub async fn list_with_counts(&self) -> Result<Vec<(UserGroupRow, i64)>> {
        let rows: Vec<(String, String, String, i64, i64, i64)> = sqlx::query_as(
            r#"SELECT g.id, g.name, g.routes, g.created_at, g.updated_at,
                      (SELECT COUNT(*) FROM user_group_members m WHERE m.group_id = g.id) AS member_count
                 FROM user_groups g
                 ORDER BY g.name ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    UserGroupRow {
                        id: r.0,
                        name: r.1,
                        routes: r.2,
                        created_at: r.3,
                        updated_at: r.4,
                    },
                    r.5,
                )
            })
            .collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<UserGroupRow>> {
        let row: Option<GroupRowTuple> = sqlx::query_as(
            "SELECT id, name, routes, created_at, updated_at FROM user_groups WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(UserGroupRow::from))
    }

    /// 该组当前成员数。
    pub async fn member_count(&self, id: &str) -> Result<i64> {
        let c: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_group_members WHERE group_id = ?1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(c.0)
    }

    /// 插入新组。名称冲突返回 DuplicateResource。
    pub async fn insert(&self, id: &str, name: &str, routes_csv: &str) -> Result<UserGroupRow> {
        let now = Utc::now().timestamp_millis();
        let res = sqlx::query(
            r#"INSERT INTO user_groups (id, name, routes, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?4)"#,
        )
        .bind(id)
        .bind(name)
        .bind(routes_csv)
        .bind(now)
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(UserGroupRow {
                id: id.to_string(),
                name: name.to_string(),
                routes: routes_csv.to_string(),
                created_at: now,
                updated_at: now,
            }),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(AppError::DuplicateResource("用户组名称".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 更新组的 name / routes(任一为 None 表示不改)。名称冲突 → DuplicateResource。
    /// 返回受影响行数(0 = 组不存在 或 无字段可改)。
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        routes_csv: Option<&str>,
    ) -> Result<u64> {
        if name.is_none() && routes_csv.is_none() {
            return Ok(0);
        }
        let now = Utc::now().timestamp_millis();
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("UPDATE user_groups SET updated_at = ");
        qb.push_bind(now);
        if let Some(n) = name {
            qb.push(", name = ");
            qb.push_bind(n);
        }
        if let Some(r) = routes_csv {
            qb.push(", routes = ");
            qb.push_bind(r);
        }
        qb.push(" WHERE id = ");
        qb.push_bind(id);
        let res = qb.build().execute(&self.pool).await;
        match res {
            Ok(r) => Ok(r.rows_affected()),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(AppError::DuplicateResource("用户组名称".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 删除组:同事务删除其所有成员关联,再删组。返回删除的组行数。
    pub async fn delete(&self, id: &str) -> Result<u64> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        sqlx::query("DELETE FROM user_group_members WHERE group_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        let res = sqlx::query("DELETE FROM user_groups WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        tx.commit()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(res.rows_affected())
    }

    /// 某用户所属**所有组**可路由网段的并集;用户未属任何组 → None
    /// (None=回退全局默认;Some(空)=有组但组无网段,仅放行 VPN 子网+站点网关)。
    pub async fn routes_for_user(&self, user_id: &str) -> Result<Option<Vec<String>>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT g.routes FROM user_group_members m
                 JOIN user_groups g ON m.group_id = g.id
                WHERE m.user_id = ?1"#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut merged: Vec<String> = Vec::new();
        for (csv,) in rows {
            for s in csv.split(',').filter(|s| !s.is_empty()) {
                let s = s.to_string();
                if !merged.contains(&s) {
                    merged.push(s);
                }
            }
        }
        Ok(Some(merged))
    }

    /// 某用户所属组 id 列表。
    pub async fn group_ids_for_user(&self, user_id: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT group_id FROM user_group_members WHERE user_id = ?1")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// 删除某用户的全部组成员关联（用户被删除时调用，避免悬挂行导致 member_count 虚高）。
    /// 返回删除行数。
    pub async fn remove_user(&self, user_id: &str) -> Result<u64> {
        let res = sqlx::query("DELETE FROM user_group_members WHERE user_id = ?1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(res.rows_affected())
    }

    /// 全量覆盖某用户的组关联:同事务清空旧关联,插入新集合(去重)。
    pub async fn set_groups(&self, user_id: &str, group_ids: &[String]) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        sqlx::query("DELETE FROM user_group_members WHERE user_id = ?1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        let mut seen: Vec<&str> = Vec::new();
        for gid in group_ids {
            if seen.contains(&gid.as_str()) {
                continue;
            }
            seen.push(gid);
            sqlx::query("INSERT INTO user_group_members (user_id, group_id) VALUES (?1, ?2)")
                .bind(user_id)
                .bind(gid)
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
            "sqlite:file:user_group_repo_test_{}?mode=memory&cache=private",
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

    async fn add_user(pool: &SqlitePool, id: &str, group: Option<&str>) {
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES (?1, ?1, ?1, 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
        if let Some(g) = group {
            sqlx::query("INSERT INTO user_group_members (user_id, group_id) VALUES (?1, ?2)")
                .bind(id)
                .bind(g)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn insert_list_and_member_count() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool.clone());
        repo.insert("g1", "ops", "10.0.0.0/8").await.unwrap();
        add_user(&pool, "u1", Some("g1")).await;
        add_user(&pool, "u2", Some("g1")).await;
        add_user(&pool, "u3", None).await;

        let list = repo.list_with_counts().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0.name, "ops");
        assert_eq!(list[0].1, 2); // member_count
    }

    #[tokio::test]
    async fn duplicate_name_rejected() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool);
        repo.insert("g1", "ops", "").await.unwrap();
        let err = repo.insert("g2", "ops", "").await.unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn delete_clears_member_group_id() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool.clone());
        repo.insert("g1", "ops", "10.0.0.0/8").await.unwrap();
        add_user(&pool, "u1", Some("g1")).await;
        assert_eq!(
            repo.routes_for_user("u1").await.unwrap(),
            Some(vec!["10.0.0.0/8".to_string()])
        );

        assert_eq!(repo.delete("g1").await.unwrap(), 1);
        // 组关联随组一并删除 → 不再解析到任何组路由
        assert_eq!(repo.routes_for_user("u1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn routes_for_user_none_when_ungrouped() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool.clone());
        add_user(&pool, "u1", None).await;
        assert_eq!(repo.routes_for_user("u1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn routes_for_user_unions_multiple_groups_and_set_replaces() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool.clone());
        repo.insert("g1", "a", "10.0.0.0/8").await.unwrap();
        repo.insert("g2", "b", "192.168.0.0/16").await.unwrap();
        add_user(&pool, "u1", None).await;
        repo.set_groups("u1", &["g1".to_string(), "g2".to_string()])
            .await
            .unwrap();
        let r = repo.routes_for_user("u1").await.unwrap().unwrap();
        assert!(r.contains(&"10.0.0.0/8".to_string()));
        assert!(r.contains(&"192.168.0.0/16".to_string()));
        assert_eq!(repo.group_ids_for_user("u1").await.unwrap().len(), 2);
        // 覆盖式设置:改为只 g2
        repo.set_groups("u1", &["g2".to_string()]).await.unwrap();
        assert_eq!(
            repo.group_ids_for_user("u1").await.unwrap(),
            vec!["g2".to_string()]
        );
    }

    #[tokio::test]
    async fn update_name_and_routes() {
        let pool = setup_pool().await;
        let repo = SqliteUserGroupRepository::new(pool);
        repo.insert("g1", "ops", "10.0.0.0/8").await.unwrap();
        assert_eq!(repo.update("g1", Some("eng"), Some("172.16.0.0/12")).await.unwrap(), 1);
        let g = repo.get("g1").await.unwrap().unwrap();
        assert_eq!(g.name, "eng");
        assert_eq!(g.routes, "172.16.0.0/12");
    }
}
