//! 网段目录业务服务:CRUD + CIDR 校验/归一化(复用
//! [`normalize_subnets`](super::peer_service::normalize_subnets))。

use uuid::Uuid;
use vpn_api_types::subnet::SubnetDto;
use vpn_core::{AppError, Result};

use crate::repositories::subnet_repo_sqlite::{SqliteSubnetRepository, SubnetRow};
use crate::services::peer_service::normalize_subnets;

#[derive(Clone)]
pub struct SubnetService {
    pub repo: SqliteSubnetRepository,
}

impl SubnetService {
    pub fn new(repo: SqliteSubnetRepository) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<SubnetDto>> {
        Ok(self
            .repo
            .list_with_usage()
            .await?
            .into_iter()
            .map(|(row, usage)| row_to_dto(row, usage as u32))
            .collect())
    }

    pub async fn create(&self, name: &str, cidr: &str) -> Result<SubnetDto> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Config("网段名称不能为空".to_string()));
        }
        let cidr = normalize_one(cidr)?;
        let id = Uuid::now_v7().to_string();
        let row = self.repo.insert(&id, name, &cidr).await?;
        // 该 CIDR 可能此前已被手填进某些组/节点 → 体现既有引用数。
        let usage = self.repo.usage_count(&row.cidr).await? as u32;
        Ok(row_to_dto(row, usage))
    }

    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        cidr: Option<&str>,
    ) -> Result<SubnetDto> {
        let name_owned = match name {
            Some(n) => {
                let t = n.trim();
                if t.is_empty() {
                    return Err(AppError::Validation("网段名称不能为空".to_string()));
                }
                Some(t.to_string())
            }
            None => None,
        };
        let cidr_owned = match cidr {
            Some(c) => Some(normalize_one(c)?),
            None => None,
        };
        let affected = self
            .repo
            .update(id, name_owned.as_deref(), cidr_owned.as_deref())
            .await?;
        if affected == 0 {
            return Err(AppError::ResourceNotFound(format!("网段不存在: {id}")));
        }
        let row = self
            .repo
            .get(id)
            .await?
            .ok_or_else(|| AppError::ResourceNotFound(format!("网段不存在: {id}")))?;
        let usage = self.repo.usage_count(&row.cidr).await? as u32;
        Ok(row_to_dto(row, usage))
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        if self.repo.delete(id).await? == 0 {
            return Err(AppError::ResourceNotFound(format!("网段不存在: {id}")));
        }
        Ok(())
    }
}

/// 校验并归一化单个 CIDR（如 192.168.1.5/24 → 192.168.1.0/24）。
fn normalize_one(cidr: &str) -> Result<String> {
    normalize_subnets(&[cidr.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Validation("网段不能为空".to_string()))
}

fn row_to_dto(r: SubnetRow, usage_count: u32) -> SubnetDto {
    SubnetDto {
        id: r.id,
        name: r.name,
        cidr: r.cidr,
        usage_count,
        created_at: r.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn svc() -> SubnetService {
        let url = format!(
            "sqlite:file:subnet_svc_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool: SqlitePool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        SubnetService::new(SqliteSubnetRepository::new(pool))
    }

    #[tokio::test]
    async fn create_normalizes_and_validates() {
        let s = svc().await;
        let d = s.create("办公网", "192.168.1.9/24").await.unwrap();
        assert_eq!(d.cidr, "192.168.1.0/24"); // 已归一化
        assert!(s.create("x", "bad").await.is_err());
        assert!(s.create("  ", "10.0.0.0/8").await.is_err());
    }

    #[tokio::test]
    async fn update_and_delete() {
        let s = svc().await;
        let d = s.create("a", "10.0.0.0/8").await.unwrap();
        let u = s
            .update(&d.id, Some("b"), Some("172.16.0.0/12"))
            .await
            .unwrap();
        assert_eq!(u.name, "b");
        assert_eq!(u.cidr, "172.16.0.0/12");
        s.delete(&d.id).await.unwrap();
        assert!(s.delete(&d.id).await.is_err());
    }
}
