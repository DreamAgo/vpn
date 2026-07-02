//! SQLite 实现的网段目录仓储。

use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use vpn_core::{AppError, Result};

#[derive(Debug, Clone)]
pub struct SubnetRow {
    pub id: String,
    pub name: String,
    pub cidr: String,
    pub created_at: i64,
    pub updated_at: i64,
}

type SubnetTuple = (String, String, String, i64, i64);

impl From<SubnetTuple> for SubnetRow {
    fn from(r: SubnetTuple) -> Self {
        SubnetRow {
            id: r.0,
            name: r.1,
            cidr: r.2,
            created_at: r.3,
            updated_at: r.4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteSubnetRepository {
    pool: SqlitePool,
}

impl SqliteSubnetRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<SubnetRow>> {
        let rows: Vec<SubnetTuple> =
            sqlx::query_as("SELECT id, name, cidr, created_at, updated_at FROM subnets ORDER BY name ASC")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows.into_iter().map(SubnetRow::from).collect())
    }

    /// 列出所有网段 + 各自被引用次数(用户组路由 + 节点路由 + 服务端 LAN 之和)。
    /// CSV 用逗号包裹后 LIKE 精确匹配整段,避免 10.0.0.0/8 误配 10.0.0.0/80 之类。
    pub async fn list_with_usage(&self) -> Result<Vec<(SubnetRow, i64)>> {
        let rows: Vec<(String, String, String, i64, i64, i64)> = sqlx::query_as(
            r#"SELECT s.id, s.name, s.cidr, s.created_at, s.updated_at,
                      (SELECT COUNT(*) FROM user_groups g
                         WHERE (',' || g.routes || ',') LIKE ('%,' || s.cidr || ',%'))
                    + (SELECT COUNT(*) FROM peers p
                         WHERE (',' || p.routed_subnets || ',') LIKE ('%,' || s.cidr || ',%'))
                    + (SELECT COUNT(*) FROM system_config c
                         WHERE c.key = 'server_routes'
                           AND (',' || c.value || ',') LIKE ('%,' || s.cidr || ',%')) AS usage_count
                 FROM subnets s
                 ORDER BY s.name ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    SubnetRow {
                        id: r.0,
                        name: r.1,
                        cidr: r.2,
                        created_at: r.3,
                        updated_at: r.4,
                    },
                    r.5,
                )
            })
            .collect())
    }

    /// 单个 CIDR 当前被引用的次数(同 list_with_usage 的口径)。
    pub async fn usage_count(&self, cidr: &str) -> Result<i64> {
        let c: (i64,) = sqlx::query_as(
            r#"SELECT (SELECT COUNT(*) FROM user_groups g
                         WHERE (',' || g.routes || ',') LIKE ('%,' || ?1 || ',%'))
                    + (SELECT COUNT(*) FROM peers p
                         WHERE (',' || p.routed_subnets || ',') LIKE ('%,' || ?1 || ',%'))
                    + (SELECT COUNT(*) FROM system_config c
                         WHERE c.key = 'server_routes'
                           AND (',' || c.value || ',') LIKE ('%,' || ?1 || ',%'))"#,
        )
        .bind(cidr)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(c.0)
    }

    pub async fn get(&self, id: &str) -> Result<Option<SubnetRow>> {
        let row: Option<SubnetTuple> =
            sqlx::query_as("SELECT id, name, cidr, created_at, updated_at FROM subnets WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(row.map(SubnetRow::from))
    }

    /// 插入。名称或 CIDR 冲突 → DuplicateResource。
    pub async fn insert(&self, id: &str, name: &str, cidr: &str) -> Result<SubnetRow> {
        let now = Utc::now().timestamp_millis();
        let res = sqlx::query(
            r#"INSERT INTO subnets (id, name, cidr, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)"#,
        )
        .bind(id)
        .bind(name)
        .bind(cidr)
        .bind(now)
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(SubnetRow {
                id: id.to_string(),
                name: name.to_string(),
                cidr: cidr.to_string(),
                created_at: now,
                updated_at: now,
            }),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(AppError::DuplicateResource("网段名称或 CIDR".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    /// 更新 name / cidr(None 表示不改)。冲突 → DuplicateResource。返回受影响行数。
    pub async fn update(&self, id: &str, name: Option<&str>, cidr: Option<&str>) -> Result<u64> {
        if name.is_none() && cidr.is_none() {
            return Ok(0);
        }
        let now = Utc::now().timestamp_millis();
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("UPDATE subnets SET updated_at = ");
        qb.push_bind(now);
        if let Some(n) = name {
            qb.push(", name = ");
            qb.push_bind(n);
        }
        if let Some(c) = cidr {
            qb.push(", cidr = ");
            qb.push_bind(c);
        }
        qb.push(" WHERE id = ");
        qb.push_bind(id);
        match qb.build().execute(&self.pool).await {
            Ok(r) => Ok(r.rows_affected()),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(AppError::DuplicateResource("网段名称或 CIDR".to_string()))
            }
            Err(e) => Err(AppError::Database(Box::new(e))),
        }
    }

    pub async fn delete(&self, id: &str) -> Result<u64> {
        let res = sqlx::query("DELETE FROM subnets WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(Box::new(e)))?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup() -> SqliteSubnetRepository {
        let url = format!(
            "sqlite:file:subnet_repo_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        SqliteSubnetRepository::new(pool)
    }

    #[tokio::test]
    async fn insert_list_update_delete() {
        let repo = setup().await;
        repo.insert("s1", "办公网", "192.168.1.0/24").await.unwrap();
        assert_eq!(repo.list().await.unwrap().len(), 1);
        assert_eq!(repo.update("s1", Some("总部"), None).await.unwrap(), 1);
        assert_eq!(repo.get("s1").await.unwrap().unwrap().name, "总部");
        assert_eq!(repo.delete("s1").await.unwrap(), 1);
        assert_eq!(repo.list().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn usage_count_counts_group_reference_exactly() {
        let url = format!(
            "sqlite:file:subnet_usage_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let repo = SqliteSubnetRepository::new(pool.clone());
        repo.insert("s1", "a", "10.0.0.0/8").await.unwrap();
        sqlx::query(
            "INSERT INTO user_groups (id,name,routes,created_at,updated_at) VALUES ('g1','grp','192.168.1.0/24,10.0.0.0/8',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(repo.usage_count("10.0.0.0/8").await.unwrap(), 1);
        assert_eq!(repo.list_with_usage().await.unwrap()[0].1, 1);
        // 未被引用的不计;且不发生子串误配。
        assert_eq!(repo.usage_count("172.16.0.0/12").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn duplicate_name_or_cidr_rejected() {
        let repo = setup().await;
        repo.insert("s1", "a", "10.0.0.0/8").await.unwrap();
        assert!(matches!(
            repo.insert("s2", "a", "10.1.0.0/16").await.unwrap_err(),
            AppError::DuplicateResource(_)
        ));
        assert!(matches!(
            repo.insert("s3", "b", "10.0.0.0/8").await.unwrap_err(),
            AppError::DuplicateResource(_)
        ));
    }
}
