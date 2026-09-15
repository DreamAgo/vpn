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
            return Err(AppError::Config("网段组名称不能为空".to_string()));
        }
        let cidr = normalize_group(cidr)?;
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
                    return Err(AppError::Validation("网段组名称不能为空".to_string()));
                }
                Some(t.to_string())
            }
            None => None,
        };
        let cidr_owned = match cidr {
            Some(c) => Some(normalize_group(c)?),
            None => None,
        };
        let affected = self
            .repo
            .update(id, name_owned.as_deref(), cidr_owned.as_deref())
            .await?;
        if affected == 0 && (name_owned.is_some() || cidr_owned.is_some()) {
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

/// 解析多行/CSV 输入，校验全部 CIDR 后归一化、去重并保存。
fn normalize_group(cidr: &str) -> Result<String> {
    let entries: Vec<String> = cidr
        .split(|c: char| c.is_whitespace() || c == ',' || c == '，')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let normalized = normalize_subnets(&entries)?;
    if normalized.is_empty() {
        return Err(AppError::Validation("网段组至少需要一个 CIDR".into()));
    }
    Ok(normalized.join(","))
}

fn row_to_dto(r: SubnetRow, usage_count: u32) -> SubnetDto {
    SubnetDto {
        id: r.id,
        name: r.name,
        cidrs: r.cidr.split(',').map(str::to_owned).collect(),
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
    async fn group_round_trip_and_invalid_update_is_atomic() {
        let s = svc().await;
        let input = "135.10.0.0/16\n10.196.184.7/24 10.196.163.126/32,10.196.176.0/24，10.196.153.0/24\r\n10.196.185.0/24\t10.179.195.0/24\n10.196.184.0/24";
        let group = s.create("贵州内网", input).await.unwrap();
        assert_eq!(group.cidrs.len(), 7);
        assert_eq!(group.cidrs[1], "10.196.184.0/24");
        assert_eq!(s.list().await.unwrap()[0].cidrs, group.cidrs);
        assert!(s
            .update(&group.id, Some("changed"), Some("10.0.0.0/8\nbad"))
            .await
            .is_err());
        assert_eq!(s.list().await.unwrap()[0].name, "贵州内网");
        assert_eq!(s.list().await.unwrap()[0].cidrs, group.cidrs);
        for input in ["", " \n,，", "0.0.0.0/0", "::1/128"] {
            assert!(s.create("invalid", input).await.is_err());
        }
        assert_eq!(
            s.update(&group.id, None, None).await.unwrap().cidrs,
            group.cidrs
        );
        let updated = s
            .update(&group.id, None, Some("10.0.0.0/8\n172.16.0.0/12"))
            .await
            .unwrap();
        assert_eq!(updated.cidrs, vec!["10.0.0.0/8", "172.16.0.0/12"]);
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
