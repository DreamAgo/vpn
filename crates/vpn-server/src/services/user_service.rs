//! 用户管理业务服务（Epic 3：admin 创建 / 列表 / 启停 / 重置密码 / 删除）。

use std::sync::Arc;

use rand::seq::SliceRandom;
use rand::Rng;
use uuid::Uuid;
use vpn_api_types::{
    user::{CreateUserResponse, ListUsersQuery, UserApprovalGrantDto, UserDto},
    Page,
};
use vpn_core::{service::PasswordHasher, AppError, Result};

use crate::{
    repositories::{
        session_repo_sqlite::SqliteSessionRepository,
        user_repo_sqlite::{OrderByColumn, SqliteUserRepository, UserListFilter, UserRow},
    },
    services::auth_service::AuthService,
};

/// 列表默认分页参数。
const DEFAULT_PAGE: u32 = 1;
const DEFAULT_PAGE_SIZE: u32 = 20;
const MAX_PAGE_SIZE: u32 = 100;
/// 自动生成密码长度。
const GENERATED_PASSWORD_LEN: usize = 12;
/// 终端数量上限的允许区间（防误配超大值撑爆 IP 池）。
pub const MAX_DEVICES_LIMIT: i64 = 100;

/// 校验终端数量上限取值（1..=MAX_DEVICES_LIMIT）。
fn validate_max_devices(n: i64) -> Result<()> {
    if !(1..=MAX_DEVICES_LIMIT).contains(&n) {
        return Err(AppError::Validation(format!(
            "终端数量上限须在 1~{MAX_DEVICES_LIMIT} 之间：{n}"
        )));
    }
    Ok(())
}

#[derive(Clone)]
pub struct UserService {
    pub user_repo: SqliteUserRepository,
    pub session_repo: SqliteSessionRepository,
    pub hasher: Arc<dyn PasswordHasher>,
}

impl UserService {
    pub fn new(
        user_repo: SqliteUserRepository,
        session_repo: SqliteSessionRepository,
        hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repo,
            session_repo,
            hasher,
        }
    }

    pub async fn update_grant_expiry(
        &self,
        actor: &str,
        user_id: &str,
        group_id: &str,
        expires_at: i64,
        expected: i64,
    ) -> Result<()> {
        // 限制到公历 9999 年以内，允许设置过去时间以立即终止授权。
        if !(1..=253402271999000).contains(&expires_at) {
            return Err(AppError::Validation("到期时间超出有效范围".into()));
        }
        let db = |e| AppError::Internal(Box::new(e));
        let mut tx = self.user_repo.pool().begin().await.map_err(db)?;
        // 先获得写锁，读取与修改使用同一事务。
        sqlx::query(
            "UPDATE access_grants SET updated_at=updated_at WHERE user_id=?1 AND group_id=?2",
        )
        .bind(user_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let before: Vec<(String, i64)> = sqlx::query_as(
            "SELECT id,expires_at FROM access_grants WHERE user_id=?1 AND group_id=?2 ORDER BY id",
        )
        .bind(user_id)
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let current = before
            .iter()
            .map(|(_, expiry)| *expiry)
            .max()
            .ok_or_else(|| AppError::Validation("该用户组没有可修改的审批授权".into()))?;
        if current != expected {
            return Err(AppError::Validation(
                "授权到期时间已变化，请刷新后重新修改".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        // 列表按组取最大值，必须同时调整该组已有授权，缩短期限才能实际生效。
        sqlx::query(
            "UPDATE access_grants SET expires_at=?3,updated_at=?4 WHERE user_id=?1 AND group_id=?2",
        )
        .bind(user_id)
        .bind(group_id)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let metadata = serde_json::json!({"target_user_id":user_id,"group_id":group_id,
            "before":before,"previous_expires_at":current,"expires_at":expires_at});
        if crate::middleware::audit_context::CONTEXT
            .try_with(|_| ())
            .is_ok()
        {
            crate::middleware::audit_context::record(
                &mut tx,
                &format!("users/{user_id}/approval-grants/{group_id}"),
                serde_json::json!({"expires_at":current}),
                serde_json::json!({"expires_at":expires_at}),
            )
            .await?;
        } else {
            sqlx::query("INSERT INTO audit_logs(id,user_id,username,action,resource,metadata,created_at) VALUES(?1,?2,(SELECT username FROM users WHERE id=?2),'grant.expiry.update',?3,?4,?5)")
            .bind(Uuid::new_v4().to_string()).bind(actor).bind(format!("users/{user_id}/approval-grants/{group_id}"))
            .bind(metadata.to_string()).bind(now).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        crate::middleware::audit_context::committed();
        Ok(())
    }

    async fn populate_approval_grants(&self, users: &mut [UserDto]) -> Result<()> {
        let ids: Vec<&str> = users.iter().map(|user| user.id.as_str()).collect();
        let grants = self.user_repo.approval_expiries(&ids).await?;
        for (user_id, group_id, group_name, expires_at) in grants {
            if let Some(user) = users.iter_mut().find(|user| user.id == user_id) {
                user.approval_grants.push(UserApprovalGrantDto {
                    group_id,
                    group_name,
                    expires_at,
                });
            }
        }
        for user in users {
            user.feishu_bindings =
                super::feishu_directory_service::bindings(self.user_repo.pool(), &user.id).await?;
        }
        Ok(())
    }

    async fn user_dto(&self, row: UserRow) -> Result<UserDto> {
        let mut dto = user_row_to_dto(row);
        self.populate_approval_grants(std::slice::from_mut(&mut dto))
            .await?;
        Ok(dto)
    }

    /// Story 3.1：创建普通用户。
    ///
    /// password 为 None 时生成 12 位强密码；明文一次性返回。
    /// max_devices 为 None 时默认 1（单终端）。
    pub async fn create_user(
        &self,
        username: &str,
        email: &str,
        password: Option<&str>,
        max_devices: Option<i64>,
    ) -> Result<CreateUserResponse> {
        let plaintext = match password {
            Some(p) => {
                AuthService::validate_password(p)?;
                p.to_string()
            }
            None => generate_password(),
        };
        let max_devices = max_devices.unwrap_or(1);
        validate_max_devices(max_devices)?;
        let hash = self.hasher.hash(&plaintext)?;
        let user_id = Uuid::now_v7().to_string();
        // role=user, must_change_password=true, status active（仓储默认 active）
        let row = self
            .user_repo
            .insert(&user_id, username, email, &hash, "user", true, max_devices)
            .await?;
        Ok(CreateUserResponse {
            user: self.user_dto(row).await?,
            initial_password: plaintext,
        })
    }

    /// Story 3.2：分页 + 搜索 + 状态筛选列表。
    pub async fn list_users(&self, query: &ListUsersQuery) -> Result<Page<UserDto>> {
        let page = query.page.unwrap_or(DEFAULT_PAGE).max(1);
        let page_size = query
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let filter = UserListFilter {
            search: query.search.clone(),
            status: query.status.clone(),
            order_by: OrderByColumn::parse(query.order_by.as_deref()),
            page,
            page_size,
        };

        let total = self.user_repo.count(&filter).await? as u64;
        let mut items: Vec<UserDto> = self
            .user_repo
            .list(&filter)
            .await?
            .into_iter()
            .map(user_row_to_dto)
            .collect();
        self.populate_approval_grants(&mut items).await?;
        Ok(Page::new(items, total, page, page_size))
    }

    pub async fn update_admin_fields(
        &self,
        user_id: &str,
        status: Option<&str>,
        max_devices: Option<i64>,
    ) -> Result<UserDto> {
        if let Some(value) = max_devices {
            validate_max_devices(value)?;
        }
        if status.is_some_and(|s| !matches!(s, "active" | "disabled")) {
            return Err(AppError::Validation("非法用户状态".into()));
        }
        self.user_repo
            .update_admin_fields(user_id, status, max_devices)
            .await?;
        let row = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        self.user_dto(row).await
    }

    /// Story 3.3：启用 / 禁用用户。禁用时撤销其所有 session。
    pub async fn update_status(&self, user_id: &str, status: &str) -> Result<UserDto> {
        if status != "active" && status != "disabled" {
            return Err(AppError::Config(format!("非法状态：{status}")));
        }
        let affected = self.user_repo.update_status(user_id, status).await?;
        if affected == 0 {
            return Err(AppError::UserNotFound);
        }
        if status == "disabled" {
            self.session_repo.revoke_all_for_user(user_id).await?;
        }
        let row = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        self.user_dto(row).await
    }

    /// 多终端模式：更新用户的终端数量上限。
    ///
    /// 调小不影响已注册终端（继续在线/心跳），仅限制后续**新**终端注册。
    pub async fn update_max_devices(&self, user_id: &str, max_devices: i64) -> Result<UserDto> {
        validate_max_devices(max_devices)?;
        let affected = self
            .user_repo
            .update_max_devices(user_id, max_devices)
            .await?;
        if affected == 0 {
            return Err(AppError::UserNotFound);
        }
        let row = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        self.user_dto(row).await
    }

    /// Story 3.4：重置用户密码。生成新强密码 + must_change_password + 撤销所有 session。
    ///
    /// admin 不允许重置自己的密码（应使用修改密码功能）。
    pub async fn reset_password(&self, actor_id: &str, target_id: &str) -> Result<String> {
        if actor_id == target_id {
            return Err(AppError::NoAccessReason("请使用修改密码功能".to_string()));
        }
        // 确认目标用户存在
        self.user_repo
            .find_by_id(target_id)
            .await?
            .ok_or(AppError::UserNotFound)?;

        let new_password = generate_password();
        let hash = self.hasher.hash(&new_password)?;
        // clear_must_change=false → must_change_password 置 1（要求下次登录改密）
        self.user_repo
            .update_password(target_id, &hash, false)
            .await?;
        self.session_repo.revoke_all_for_user(target_id).await?;
        Ok(new_password)
    }

    /// Story 3.5：删除用户（级联删 user + sessions，单事务）。
    ///
    /// admin 不允许删除自己。
    pub async fn delete_user(&self, actor_id: &str, target_id: &str) -> Result<()> {
        if actor_id == target_id {
            return Err(AppError::NoAccessReason("无法删除自己的账号".to_string()));
        }
        // TODO(Epic 4): 级联删除该用户 peers + WireGuard runtime 清理
        let affected = self.user_repo.delete_with_sessions(target_id).await?;
        if affected == 0 {
            return Err(AppError::UserNotFound);
        }
        Ok(())
    }
}

/// UserRow → UserDto（剥离 password_hash 等敏感字段）。
fn user_row_to_dto(row: UserRow) -> UserDto {
    UserDto {
        id: row.id,
        username: row.username,
        email: row.email,
        role: row.role,
        status: row.status,
        must_change_password: row.must_change_password,
        last_login_at: row.last_login_at,
        group_ids: row.group_ids,
        max_devices: row.max_devices,
        access_mode: row.access_mode,
        approval_grants: Vec::new(),
        feishu_bindings: Vec::new(),
        created_at: row.created_at,
    }
}

/// 生成 12 位强密码：含大小写字母 + 数字 + 特殊字符，且满足 validate_password
/// （至少含字母与数字）。
fn generate_password() -> String {
    const LOWER: &[u8] = b"abcdefghijkmnpqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGITS: &[u8] = b"23456789";
    const SPECIAL: &[u8] = b"!@#$%^&*-_=+";

    let mut rng = rand::rngs::OsRng;
    let mut chars: Vec<u8> = Vec::with_capacity(GENERATED_PASSWORD_LEN);

    // 先各放一个，保证至少含大小写字母、数字、特殊字符。
    chars.push(LOWER[rng.gen_range(0..LOWER.len())]);
    chars.push(UPPER[rng.gen_range(0..UPPER.len())]);
    chars.push(DIGITS[rng.gen_range(0..DIGITS.len())]);
    chars.push(SPECIAL[rng.gen_range(0..SPECIAL.len())]);

    let all: Vec<u8> = [LOWER, UPPER, DIGITS, SPECIAL].concat();
    while chars.len() < GENERATED_PASSWORD_LEN {
        chars.push(all[rng.gen_range(0..all.len())]);
    }
    // 打乱，避免固定的字符类别位置。
    chars.shuffle(&mut rng);

    String::from_utf8(chars).expect("password chars are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Argon2Hasher;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn setup_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:user_service_test_{}?mode=memory&cache=private",
            Uuid::new_v4()
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

    fn service(pool: SqlitePool) -> UserService {
        UserService::new(
            SqliteUserRepository::new(pool.clone()),
            SqliteSessionRepository::new(pool),
            Arc::new(Argon2Hasher::new()),
        )
    }

    #[tokio::test]
    async fn expiry_edit_updates_all_same_group_grants_and_audits_atomically() {
        let pool = setup_pool().await;
        let svc = service(pool.clone());
        let user = svc
            .create_user("alice", "alice@example.com", None, None)
            .await
            .unwrap()
            .user;
        for group in ["g1", "g2"] {
            sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES(?1,?1,'',0,0)")
                .bind(group).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO user_group_members(user_id,group_id) VALUES(?1,'g1')")
            .bind(&user.id)
            .execute(&pool)
            .await
            .unwrap();
        for (id, group, expiry) in [("a", "g1", 1000), ("b", "g1", 2000), ("c", "g2", 3000)] {
            sqlx::query("INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,created_at,updated_at) VALUES(?1,?1,?2,?3,?4,0,0)")
                .bind(id).bind(&user.id).bind(group).bind(expiry).execute(&pool).await.unwrap();
        }
        svc.update_grant_expiry(&user.id, &user.id, "g1", 500, 2000)
            .await
            .unwrap();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT id,expires_at FROM access_grants ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![("a".into(), 500), ("b".into(), 500), ("c".into(), 3000)]
        );
        assert!(svc
            .update_grant_expiry(&user.id, &user.id, "g1", 9000, 2000)
            .await
            .is_err());
        assert!(svc
            .update_grant_expiry(&user.id, &user.id, "missing", 9000, 500)
            .await
            .is_err());
        assert!(svc
            .update_grant_expiry(&user.id, &user.id, "g1", 0, 500)
            .await
            .is_err());
        let audit: (String, String) = sqlx::query_as(
            "SELECT user_id,metadata FROM audit_logs WHERE action='grant.expiry.update'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit.0, user.id);
        let data: serde_json::Value = serde_json::from_str(&audit.1).unwrap();
        assert_eq!(data["previous_expires_at"], 2000);
        assert_eq!(data["expires_at"], 500);
        assert_eq!(data["before"].as_array().unwrap().len(), 2);
        svc.update_grant_expiry(&user.id, &user.id, "g1", 9000, 500)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user_group_members")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id=?1")
                .bind(&user.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "active"
        );
    }

    #[tokio::test]
    async fn user_list_exposes_group_expiries_without_changing_manual_membership() {
        let pool = setup_pool().await;
        let svc = service(pool.clone());
        let user = svc
            .create_user("approval-user", "approval@example.com", None, None)
            .await
            .unwrap()
            .user;
        let legacy = svc
            .create_user("legacy-user", "legacy@example.com", None, None)
            .await
            .unwrap()
            .user;
        assert_eq!(legacy.access_mode, "legacy");
        assert!(legacy.approval_grants.is_empty());
        sqlx::query("UPDATE users SET access_mode='approval_required' WHERE id=?1")
            .bind(&user.id)
            .execute(&pool)
            .await
            .unwrap();
        for id in ["manual", "approved", "expired"] {
            sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES(?1,?1,'',0,0)")
                .bind(id).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO user_group_members(user_id,group_id) VALUES(?1,'manual')")
            .bind(&user.id)
            .execute(&pool)
            .await
            .unwrap();
        let future = chrono::Utc::now().timestamp_millis() + 86_400_000;
        for (id, group, expiry) in [
            ("old", "approved", 1),
            ("renewal", "approved", future),
            ("past", "expired", 2),
        ] {
            sqlx::query("INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,created_at,updated_at) VALUES(?1,?1,?2,?3,?4,0,0)")
                .bind(id).bind(&user.id).bind(group).bind(expiry).execute(&pool).await.unwrap();
        }
        let query: ListUsersQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        let page = svc.list_users(&query).await.unwrap();
        let json = serde_json::to_value(&page).unwrap();
        // Verify the actual list response, including users without grants.
        let dto = page.items.iter().find(|u| u.id == user.id).unwrap();
        assert_eq!(dto.access_mode, "approval_required");
        assert_eq!(dto.group_ids, vec!["manual"]);
        assert_eq!(dto.approval_grants.len(), 2);
        assert_eq!(dto.approval_grants[0].group_name, "approved");
        assert_eq!(dto.approval_grants[0].expires_at, future);
        assert_eq!(dto.approval_grants[1].expires_at, 2);
        assert!(page
            .items
            .iter()
            .find(|u| u.id == legacy.id)
            .unwrap()
            .approval_grants
            .is_empty());
        assert!(!json.to_string().contains("password_hash"));
        let updated = svc.update_max_devices(&user.id, 2).await.unwrap();
        assert_eq!(updated.approval_grants.len(), 2);
        let filtered: ListUsersQuery =
            serde_json::from_value(serde_json::json!({"search":"legacy-user"})).unwrap();
        let page = svc.list_users(&filtered).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].approval_grants.is_empty());
    }

    #[test]
    fn generated_password_is_strong() {
        for _ in 0..50 {
            let p = generate_password();
            assert_eq!(p.len(), GENERATED_PASSWORD_LEN);
            assert!(p.chars().any(|c| c.is_lowercase()));
            assert!(p.chars().any(|c| c.is_uppercase()));
            assert!(p.chars().any(|c| c.is_ascii_digit()));
            assert!(p.chars().any(|c| !c.is_alphanumeric()));
            // 必须通过 validate_password（含字母 + 数字 + ≥8 位）
            assert!(AuthService::validate_password(&p).is_ok());
        }
    }

    #[tokio::test]
    async fn create_user_generates_password_when_omitted() {
        let svc = service(setup_pool().await);
        let resp = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        assert_eq!(resp.user.username, "alice");
        assert_eq!(resp.user.role, "user");
        assert_eq!(resp.user.status, "active");
        assert!(resp.user.must_change_password);
        assert_eq!(resp.initial_password.len(), GENERATED_PASSWORD_LEN);
        assert!(AuthService::validate_password(&resp.initial_password).is_ok());
    }

    #[tokio::test]
    async fn create_user_defaults_to_single_device() {
        let svc = service(setup_pool().await);
        let resp = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        assert_eq!(resp.user.max_devices, 1);
    }

    #[tokio::test]
    async fn create_user_with_max_devices_and_update() {
        let svc = service(setup_pool().await);
        let resp = svc
            .create_user("alice", "alice@e.com", None, Some(3))
            .await
            .unwrap();
        assert_eq!(resp.user.max_devices, 3);

        let dto = svc.update_max_devices(&resp.user.id, 5).await.unwrap();
        assert_eq!(dto.max_devices, 5);

        // 非法取值（<1 / 超上限）拒绝。
        assert!(matches!(
            svc.update_max_devices(&resp.user.id, 0).await.unwrap_err(),
            AppError::Validation(_)
        ));
        assert!(matches!(
            svc.create_user("bob", "bob@e.com", None, Some(MAX_DEVICES_LIMIT + 1))
                .await
                .unwrap_err(),
            AppError::Validation(_)
        ));
        // 未知用户。
        assert!(matches!(
            svc.update_max_devices("missing", 2).await.unwrap_err(),
            AppError::UserNotFound
        ));
    }

    #[tokio::test]
    async fn create_user_uses_provided_password() {
        let svc = service(setup_pool().await);
        let resp = svc
            .create_user("bob", "bob@e.com", Some("Sup3rSecret"), None)
            .await
            .unwrap();
        assert_eq!(resp.initial_password, "Sup3rSecret");
    }

    #[tokio::test]
    async fn create_user_rejects_weak_provided_password() {
        let svc = service(setup_pool().await);
        let err = svc
            .create_user("bob", "bob@e.com", Some("short"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::PasswordTooWeak(_)));
    }

    #[tokio::test]
    async fn create_user_duplicate_returns_duplicate_resource() {
        let svc = service(setup_pool().await);
        svc.create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let err = svc
            .create_user("alice", "other@e.com", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
    }

    #[tokio::test]
    async fn disable_user_revokes_sessions() {
        let pool = setup_pool().await;
        let svc = service(pool.clone());
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let uid = created.user.id;
        // 给该用户插入一个活跃 session
        svc.session_repo
            .create("s1", &uid, "hash-1", None, None, 9_999_999_999_999)
            .await
            .unwrap();

        let dto = svc.update_status(&uid, "disabled").await.unwrap();
        assert_eq!(dto.status, "disabled");
        assert!(svc
            .session_repo
            .find_active_by_token_hash("hash-1", 0)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn update_status_unknown_user_returns_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.update_status("missing", "disabled").await.unwrap_err();
        assert!(matches!(err, AppError::UserNotFound));
    }

    #[tokio::test]
    async fn update_status_rejects_invalid_status() {
        let svc = service(setup_pool().await);
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let err = svc
            .update_status(&created.user.id, "banned")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }

    #[tokio::test]
    async fn reset_password_rejects_self() {
        let svc = service(setup_pool().await);
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let uid = created.user.id;
        let err = svc.reset_password(&uid, &uid).await.unwrap_err();
        assert!(matches!(err, AppError::NoAccessReason(_)));
    }

    #[tokio::test]
    async fn reset_password_generates_new_and_revokes_sessions() {
        let svc = service(setup_pool().await);
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let uid = created.user.id;
        svc.session_repo
            .create("s1", &uid, "hash-1", None, None, 9_999_999_999_999)
            .await
            .unwrap();

        let new_password = svc.reset_password("admin-id", &uid).await.unwrap();
        assert!(AuthService::validate_password(&new_password).is_ok());
        // session 被撤销
        assert!(svc
            .session_repo
            .find_active_by_token_hash("hash-1", 0)
            .await
            .unwrap()
            .is_none());
        // must_change_password 重新置位
        let row = svc.user_repo.find_by_id(&uid).await.unwrap().unwrap();
        assert!(row.must_change_password);
    }

    #[tokio::test]
    async fn reset_password_unknown_user_returns_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.reset_password("admin-id", "missing").await.unwrap_err();
        assert!(matches!(err, AppError::UserNotFound));
    }

    #[tokio::test]
    async fn delete_user_rejects_self() {
        let svc = service(setup_pool().await);
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let uid = created.user.id;
        let err = svc.delete_user(&uid, &uid).await.unwrap_err();
        assert!(matches!(err, AppError::NoAccessReason(_)));
    }

    #[tokio::test]
    async fn delete_user_removes_record() {
        let svc = service(setup_pool().await);
        let created = svc
            .create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        let uid = created.user.id;
        svc.delete_user("admin-id", &uid).await.unwrap();
        assert!(svc.user_repo.find_by_id(&uid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_unknown_user_returns_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.delete_user("admin-id", "missing").await.unwrap_err();
        assert!(matches!(err, AppError::UserNotFound));
    }

    #[tokio::test]
    async fn list_users_applies_defaults_and_search() {
        let svc = service(setup_pool().await);
        svc.create_user("alice", "alice@e.com", None, None)
            .await
            .unwrap();
        svc.create_user("bob", "bob@e.com", None, None)
            .await
            .unwrap();

        let page = svc
            .list_users(&ListUsersQuery {
                page: None,
                page_size: None,
                search: None,
                status: None,
                order_by: None,
            })
            .await
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.page, 1);
        assert_eq!(page.page_size, 20);

        let page = svc
            .list_users(&ListUsersQuery {
                page: None,
                page_size: None,
                search: Some("ali".to_string()),
                status: None,
                order_by: None,
            })
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].username, "alice");
    }
}
