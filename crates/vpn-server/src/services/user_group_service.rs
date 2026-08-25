//! 用户组业务服务:CRUD + 把用户分配到组。
//!
//! 组持有一组"可路由网段"(CIDR);成员注册时由 [`PeerService`](super::PeerService)
//! 据此计算 allowed_routes(访问控制)。网段校验/归一化复用
//! [`normalize_subnets`](super::peer_service::normalize_subnets)。

use ipnet::Ipv4Net;
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
    route_policy: Option<GroupRoutePolicy>,
}

#[derive(Clone)]
struct GroupRoutePolicy {
    vpn_subnet: Ipv4Net,
}

impl UserGroupService {
    pub fn new(group_repo: SqliteUserGroupRepository, user_repo: SqliteUserRepository) -> Self {
        Self {
            group_repo,
            user_repo,
            route_policy: None,
        }
    }

    pub fn with_route_policy(mut self, vpn_subnet: Ipv4Net) -> Self {
        self.route_policy = Some(GroupRoutePolicy { vpn_subnet });
        self
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
        let normalized = self.normalize_group_routes(routes).await?;
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
            Some(r) => Some(self.normalize_group_routes(r).await?.join(",")),
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

    async fn normalize_group_routes(&self, routes: &[String]) -> Result<Vec<String>> {
        let normalized = normalize_subnets(routes)?;
        let Some(policy) = &self.route_policy else {
            return Ok(normalized);
        };
        let nets = normalized
            .iter()
            .filter_map(|route| route.parse::<Ipv4Net>().ok())
            .collect::<Vec<_>>();
        if let Some(route) = nets.iter().find(|route| policy.vpn_subnet.contains(*route)) {
            return Err(AppError::Validation(format!(
                "用户组路由 {route} 不得等于或位于 VPN 基础网段 {} 内",
                policy.vpn_subnet
            )));
        }
        Ok(normalized)
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

    async fn setup_with_vpn_subnet(vpn_subnet: &str) -> UserGroupService {
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
        .with_route_policy(vpn_subnet.parse().unwrap())
    }

    async fn setup() -> UserGroupService {
        setup_with_vpn_subnet("10.8.0.0/24").await
    }

    #[tokio::test]
    async fn create_normalizes_routes() {
        let svc = setup().await;
        let g = svc
            .create(
                "ops",
                &["172.31.100.5/24".to_string(), "100.64.0.0/10".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(g.name, "ops");
        assert!(g.routes.contains(&"172.31.100.0/24".to_string())); // 已归一化
        assert!(g.routes.contains(&"100.64.0.0/10".to_string()));
        assert_eq!(g.member_count, 0);
    }

    #[tokio::test]
    async fn create_rejects_blank_name_and_bad_cidr() {
        let svc = setup().await;
        assert!(svc.create("  ", &[]).await.is_err());
        assert!(svc.create("g", &["not-a-cidr".to_string()]).await.is_err());
    }

    #[tokio::test]
    async fn create_enforces_asymmetric_vpn_route_matrix() {
        let svc = setup().await;

        for (name, route) in [
            ("vpn-supernet", "10.0.0.0/8"),
            ("vpn-near-supernet", "10.8.0.0/23"),
            ("site-exact", "192.168.50.0/24"),
        ] {
            let group = svc.create(name, &[route.to_string()]).await.unwrap();
            assert_eq!(group.routes, vec![route.to_string()]);
        }

        for (name, route) in [
            ("vpn-exact", "10.8.0.0/24"),
            ("vpn-subnet", "10.8.0.128/25"),
            ("vpn-host", "10.8.0.3/32"),
        ] {
            let error = svc.create(name, &[route.to_string()]).await.unwrap_err();
            let AppError::Validation(message) = error else {
                panic!("expected validation error, got {error:?}");
            };
            assert!(message.contains(route));
            assert!(message.contains("10.8.0.0/24"));
            assert!(svc
                .group_repo
                .list_with_counts()
                .await
                .unwrap()
                .iter()
                .all(|(group, _)| group.name != name));
        }

        let error = svc
            .create(
                "mixed",
                &["192.168.70.0/24".to_string(), "10.8.0.128/25".to_string()],
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Validation(_)));
        assert!(svc
            .group_repo
            .list_with_counts()
            .await
            .unwrap()
            .iter()
            .all(|(group, _)| group.name != "mixed"));

        let error = svc
            .create("default", &["0.0.0.0/0".to_string()])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("暂不支持全隧道网段"));
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
    async fn update_enforces_asymmetric_vpn_route_matrix_without_partial_writes() {
        let svc = setup().await;
        let group = svc
            .create("ops", &["192.168.50.0/24".to_string()])
            .await
            .unwrap();

        for route in ["10.0.0.0/8", "10.8.0.0/23", "192.168.60.0/24"] {
            let updated = svc
                .update(&group.id, None, Some(&[route.to_string()]))
                .await
                .unwrap();
            assert_eq!(updated.routes, vec![route.to_string()]);
        }

        let last_valid_route = "192.168.60.0/24";
        for route in ["10.8.0.0/24", "10.8.0.128/25", "10.8.0.3/32"] {
            let error = svc
                .update(
                    &group.id,
                    Some("must-not-persist"),
                    Some(&[route.to_string()]),
                )
                .await
                .unwrap_err();
            let AppError::Validation(message) = error else {
                panic!("expected validation error, got {error:?}");
            };
            assert!(message.contains(route));
            assert!(message.contains("10.8.0.0/24"));
            let stored = svc.group_repo.get(&group.id).await.unwrap().unwrap();
            assert_eq!(stored.name, "ops");
            assert_eq!(stored.routes, last_valid_route);
        }

        let error = svc
            .update(
                &group.id,
                Some("must-not-persist"),
                Some(&["0.0.0.0/0".to_string()]),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("暂不支持全隧道网段"));
        let stored = svc.group_repo.get(&group.id).await.unwrap().unwrap();
        assert_eq!(stored.name, "ops");
        assert_eq!(stored.routes, last_valid_route);

        let error = svc
            .update(
                &group.id,
                Some("mixed-must-not-persist"),
                Some(&["192.168.70.0/24".to_string(), "10.8.0.128/25".to_string()]),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Validation(_)));
        let stored = svc.group_repo.get(&group.id).await.unwrap().unwrap();
        assert_eq!(stored.name, "ops");
        assert_eq!(stored.routes, last_valid_route);
    }

    #[tokio::test]
    async fn runtime_10_9_policy_allows_supernet_and_rejects_exact_or_narrower_routes() {
        let svc = setup_with_vpn_subnet("10.9.0.0/24").await;

        let created = svc
            .create("vpn-supernet", &["10.0.0.0/8".to_string()])
            .await
            .unwrap();
        assert_eq!(created.routes, vec!["10.0.0.0/8".to_string()]);

        for (name, route) in [
            ("vpn-exact-10-9", "10.9.0.0/24"),
            ("vpn-subnet-10-9", "10.9.0.128/25"),
            ("vpn-host-10-9", "10.9.0.3/32"),
        ] {
            let error = svc.create(name, &[route.to_string()]).await.unwrap_err();
            let AppError::Validation(message) = error else {
                panic!("expected validation error, got {error:?}");
            };
            assert!(message.contains(route));
            assert!(message.contains("10.9.0.0/24"));
        }

        let updated = svc
            .update(&created.id, None, Some(&["10.0.0.0/8".to_string()]))
            .await
            .unwrap();
        assert_eq!(updated.routes, vec!["10.0.0.0/8".to_string()]);

        for route in ["10.9.0.0/24", "10.9.0.128/25", "10.9.0.3/32"] {
            let error = svc
                .update(&created.id, None, Some(&[route.to_string()]))
                .await
                .unwrap_err();
            let AppError::Validation(message) = error else {
                panic!("expected validation error, got {error:?}");
            };
            assert!(message.contains(route));
            assert!(message.contains("10.9.0.0/24"));
            let stored = svc.group_repo.get(&created.id).await.unwrap().unwrap();
            assert_eq!(stored.routes, "10.0.0.0/8");
        }
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
            .insert("u1", "alice", "a@e.com", "h", "user", false, 1)
            .await
            .unwrap();
        assert!(matches!(
            svc.set_user_groups("u1", &["nope".to_string()])
                .await
                .unwrap_err(),
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
