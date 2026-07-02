//! 用户组业务服务:CRUD + 把用户分配到组。
//!
//! 组持有一组"可路由网段"(CIDR);成员注册时由 [`PeerService`](super::PeerService)
//! 据此计算 allowed_routes(访问控制)。网段校验/归一化复用
//! [`normalize_subnets`](super::peer_service::normalize_subnets)。

use uuid::Uuid;
use vpn_api_types::group::UserGroupDto;
use vpn_core::{AppError, Result};

use crate::repositories::{
    user_group_repo_sqlite::{SqliteUserGroupRepository, UserGroupRow},
    user_repo_sqlite::SqliteUserRepository,
};
use crate::services::peer_service::normalize_subnets;

#[derive(Clone)]
pub struct UserGroupService {
    pub group_repo: SqliteUserGroupRepository,
    pub user_repo: SqliteUserRepository,
}

impl UserGroupService {
    pub fn new(group_repo: SqliteUserGroupRepository, user_repo: SqliteUserRepository) -> Self {
        Self {
            group_repo,
            user_repo,
        }
    }

    /// 列出所有组(含成员数)。
    pub async fn list(&self) -> Result<Vec<UserGroupDto>> {
        Ok(self
            .group_repo
            .list_with_counts()
            .await?
            .into_iter()
            .map(|(row, count)| row_to_dto(row, count as u32))
            .collect())
    }

    /// 创建组。名称去空白且非空;routes 校验/归一化为 CIDR。名称冲突 → DuplicateResource。
    pub async fn create(&self, name: &str, routes: &[String]) -> Result<UserGroupDto> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Validation("用户组名称不能为空".to_string()));
        }
        let normalized = normalize_subnets(routes)?;
        let id = Uuid::now_v7().to_string();
        let row = self
            .group_repo
            .insert(&id, name, &normalized.join(","))
            .await?;
        Ok(row_to_dto(row, 0))
    }

    /// 更新组的 name / routes(None 表示不改)。组不存在 → Config 错误。
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        routes: Option<&[String]>,
    ) -> Result<UserGroupDto> {
        // 名称去空白校验
        let name_owned = match name {
            Some(n) => {
                let t = n.trim();
                if t.is_empty() {
                    return Err(AppError::Validation("用户组名称不能为空".to_string()));
                }
                Some(t.to_string())
            }
            None => None,
        };
        let routes_csv = match routes {
            Some(r) => Some(normalize_subnets(r)?.join(",")),
            None => None,
        };
        let affected = self
            .group_repo
            .update(id, name_owned.as_deref(), routes_csv.as_deref())
            .await?;
        if affected == 0 {
            return Err(AppError::ResourceNotFound(format!("用户组不存在: {id}")));
        }
        let row = self
            .group_repo
            .get(id)
            .await?
            .ok_or_else(|| AppError::ResourceNotFound(format!("用户组不存在: {id}")))?;
        let count = self.group_repo.member_count(id).await?;
        Ok(row_to_dto(row, count as u32))
    }

    /// 删除组(同事务把成员 group_id 清空)。组不存在 → Config 错误。
    pub async fn delete(&self, id: &str) -> Result<()> {
        let affected = self.group_repo.delete(id).await?;
        if affected == 0 {
            return Err(AppError::ResourceNotFound(format!("用户组不存在: {id}")));
        }
        Ok(())
    }

    /// 删除某用户的全部组关联（用户被删除时联动清理，避免悬挂成员行）。
    pub async fn remove_user_from_groups(&self, user_id: &str) -> Result<()> {
        self.group_repo.remove_user(user_id).await?;
        Ok(())
    }

    /// 全量设置某用户所属组(空列表=取消所有分组)。用户/组不存在 → 错误;组 id 去重。
    pub async fn set_user_groups(&self, user_id: &str, group_ids: &[String]) -> Result<()> {
        if !self.user_repo.exists(user_id).await? {
            return Err(AppError::UserNotFound);
        }
        for gid in group_ids {
            self.group_repo
                .get(gid)
                .await?
                .ok_or_else(|| AppError::ResourceNotFound(format!("用户组不存在: {gid}")))?;
        }
        self.group_repo.set_groups(user_id, group_ids).await?;
        Ok(())
    }
}

fn row_to_dto(row: UserGroupRow, member_count: u32) -> UserGroupDto {
    UserGroupDto {
        id: row.id,
        name: row.name,
        routes: row
            .routes
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        member_count,
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn setup() -> UserGroupService {
        let url = format!(
            "sqlite:file:user_group_svc_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool: SqlitePool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        UserGroupService::new(
            SqliteUserGroupRepository::new(pool.clone()),
            SqliteUserRepository::new(pool),
        )
    }

    #[tokio::test]
    async fn create_normalizes_routes() {
        let svc = setup().await;
        let g = svc
            .create("ops", &["172.31.100.5/24".to_string(), "10.0.0.0/8".to_string()])
            .await
            .unwrap();
        assert_eq!(g.name, "ops");
        assert!(g.routes.contains(&"172.31.100.0/24".to_string())); // 已归一化
        assert!(g.routes.contains(&"10.0.0.0/8".to_string()));
        assert_eq!(g.member_count, 0);
    }

    #[tokio::test]
    async fn create_rejects_blank_name_and_bad_cidr() {
        let svc = setup().await;
        assert!(svc.create("  ", &[]).await.is_err());
        assert!(svc.create("g", &["not-a-cidr".to_string()]).await.is_err());
    }

    #[tokio::test]
    async fn update_and_delete() {
        let svc = setup().await;
        let g = svc.create("ops", &[]).await.unwrap();
        let updated = svc
            .update(&g.id, Some("eng"), Some(&["192.168.5.0/24".to_string()]))
            .await
            .unwrap();
        assert_eq!(updated.name, "eng");
        assert_eq!(updated.routes, vec!["192.168.5.0/24".to_string()]);
        svc.delete(&g.id).await.unwrap();
        assert!(svc.delete(&g.id).await.is_err()); // 已删,再删报错
    }

    #[tokio::test]
    async fn set_groups_validates_user_and_group_then_assigns() {
        let svc = setup().await;
        // 用户不存在 → UserNotFound
        assert!(matches!(
            svc.set_user_groups("u1", &[]).await.unwrap_err(),
            AppError::UserNotFound
        ));
        // 建用户后,引用不存在的组 → Config
        svc.user_repo
            .insert("u1", "alice", "a@e.com", "h", "user", false)
            .await
            .unwrap();
        assert!(matches!(
            svc.set_user_groups("u1", &["nope".to_string()]).await.unwrap_err(),
            AppError::ResourceNotFound(_)
        ));
        // 正常分配两个组(多组)
        let g1 = svc.create("g1", &[]).await.unwrap();
        let g2 = svc.create("g2", &[]).await.unwrap();
        svc.set_user_groups("u1", &[g1.id.clone(), g2.id.clone()])
            .await
            .unwrap();
        assert_eq!(
            svc.group_repo.group_ids_for_user("u1").await.unwrap().len(),
            2
        );
    }
}
