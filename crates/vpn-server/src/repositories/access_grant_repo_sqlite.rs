//! 飞书审批 durable inbox 与逐实例授权的 SQLite 事实源。

use chrono::Utc;
use sqlx::SqlitePool;
use vpn_core::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueResult {
    Inserted,
    Duplicate,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct InboxRow {
    pub event_id: String,
    pub instance_code: String,
    pub payload: String,
    pub attempts: i64,
}

#[derive(Debug, Clone)]
pub struct ApprovalIdentity<'a> {
    pub subject: &'a str,
    pub email: &'a str,
    pub preferred_username: &'a str,
    pub username_suffix: &'a str,
    pub new_user_id: &'a str,
    pub password_hash: &'a str,
}

#[derive(Debug, Clone)]
pub struct ApprovedGrant<'a> {
    pub instance_code: &'a str,
    pub group_ids: Vec<String>,
    pub expires_at: i64,
    pub reason: &'a str,
    pub identity: ApprovalIdentity<'a>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ApprovalMailRow {
    pub instance_code: String,
    pub recipient: String,
    pub body: String,
    pub expires_at: i64,
    pub done: bool,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SqliteAccessGrantRepository {
    pool: SqlitePool,
}

impl SqliteAccessGrantRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn claim_mail(&self) -> Result<Option<ApprovalMailRow>> {
        let now = Utc::now().timestamp_millis();
        sqlx::query_as("UPDATE approval_mail_outbox SET attempts=attempts+1,next_attempt_at=?1 WHERE instance_code=(SELECT instance_code FROM approval_mail_outbox WHERE done=0 AND next_attempt_at<=?2 ORDER BY next_attempt_at,instance_code LIMIT 1) RETURNING *")
            .bind(now + 300_000).bind(now).fetch_optional(&self.pool).await.map_err(db)
    }

    pub async fn finish_mail(&self, row: &ApprovalMailRow, error: Option<&str>) -> Result<()> {
        let delay = 30_000_i64 * (1_i64 << row.attempts.min(7));
        sqlx::query("UPDATE approval_mail_outbox SET done=?1,last_error=?2,next_attempt_at=?3 WHERE instance_code=?4 AND attempts=?5 AND done=0")
            .bind(error.is_none()).bind(error)
            .bind(Utc::now().timestamp_millis() + delay.min(3_600_000))
            .bind(&row.instance_code).bind(row.attempts).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    pub async fn enqueue(
        &self,
        event_id: &str,
        instance_code: &str,
        payload_hash: &str,
        payload: &str,
    ) -> Result<EnqueueResult> {
        let now = Utc::now().timestamp_millis();
        let inserted = sqlx::query(
            r#"INSERT OR IGNORE INTO feishu_approval_inbox
               (event_id, instance_code, payload_hash, payload, next_attempt_at, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)"#,
        )
        .bind(event_id)
        .bind(instance_code)
        .bind(payload_hash)
        .bind(payload)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        if inserted.rows_affected() == 1 {
            return Ok(EnqueueResult::Inserted);
        }
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT payload_hash FROM feishu_approval_inbox WHERE event_id = ?1")
                .bind(event_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        if let Some((hash,)) = existing {
            return Ok(if hash == payload_hash {
                EnqueueResult::Duplicate
            } else {
                EnqueueResult::Conflict
            });
        }
        let instance: Option<(String,)> = sqlx::query_as(
            "SELECT payload_hash FROM feishu_approval_inbox WHERE instance_code = ?1",
        )
        .bind(instance_code)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(match instance {
            Some((hash,)) if hash == payload_hash => EnqueueResult::Duplicate,
            Some(_) => EnqueueResult::Conflict,
            None => EnqueueResult::Conflict,
        })
    }

    /// 原子认领一条到期任务；processing 崩溃租约两分钟后可重新认领。
    pub async fn claim_due(&self) -> Result<Option<InboxRow>> {
        let now = Utc::now().timestamp_millis();
        // 单任务最多包含多次飞书 HTTP 请求、密码哈希和 ACL 刷新；给正常处理留出
        // 充足租约，避免滚动重启期间第二个 worker 过早重复认领。
        let stale = now - 120_000;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let row: Option<(String, String, String, i64)> = sqlx::query_as(
            r#"SELECT event_id, instance_code, payload, attempts
                 FROM feishu_approval_inbox
                WHERE ((status IN ('pending','retry') AND next_attempt_at <= ?1)
                    OR (status = 'processing' AND updated_at <= ?2))
                ORDER BY created_at LIMIT 1"#,
        )
        .bind(now)
        .bind(stale)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let Some((event_id, instance_code, payload, attempts)) = row else {
            tx.commit().await.map_err(db)?;
            return Ok(None);
        };
        let updated = sqlx::query(
            r#"UPDATE feishu_approval_inbox
                  SET status = 'processing', attempts = attempts + 1, updated_at = ?2
                WHERE event_id = ?1 AND ((status IN ('pending','retry') AND next_attempt_at <= ?2)
                   OR (status = 'processing' AND updated_at <= ?3))"#,
        )
        .bind(&event_id)
        .bind(now)
        .bind(stale)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(InboxRow {
            event_id,
            instance_code,
            payload,
            attempts: attempts + 1,
        }))
    }

    pub async fn finish(&self, event_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE feishu_approval_inbox SET status='done', last_error=NULL, updated_at=?2 WHERE event_id=?1",
        )
        .bind(event_id)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn reject(&self, event_id: &str, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE feishu_approval_inbox SET status='rejected', last_error=?2, updated_at=?3 WHERE event_id=?1",
        )
        .bind(event_id)
        .bind(truncate(error))
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn retry(&self, event_id: &str, attempts: i64, error: &str) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let delay = 5_000_i64.saturating_mul(1_i64 << attempts.clamp(0, 8));
        sqlx::query(
            r#"UPDATE feishu_approval_inbox SET status='retry', next_attempt_at=?2,
               last_error=?3, updated_at=?4 WHERE event_id=?1"#,
        )
        .bind(event_id)
        .bind(now + delay.min(15 * 60_000))
        .bind(truncate(error))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// 同一事务完成身份绑定/建号/逐审批授权；既有账号保持 legacy，授权不碰人工组。
    pub async fn apply_approved(&self, grant: ApprovedGrant<'_>) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let group_ids: std::collections::BTreeSet<&str> =
            grant.group_ids.iter().map(String::as_str).collect();
        if group_ids.is_empty() {
            return Err(AppError::Validation("至少选择一个用户组".into()));
        }
        for group_id in &group_ids {
            let group_exists: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM user_groups WHERE id=?1")
                    .bind(group_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
            if group_exists.0 != 1 {
                return Err(AppError::Validation("审批选择的用户组不存在".into()));
            }
        }
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT user_id FROM external_identities WHERE provider='feishu' AND subject=?1",
        )
        .bind(grant.identity.subject)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let user_id = if let Some((id,)) = existing {
            let status: Option<(String,)> = sqlx::query_as("SELECT status FROM users WHERE id=?1")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
            match status {
                Some((status,)) if status == "active" => id,
                Some(_) => return Err(AppError::AccountDisabled),
                None => return Err(AppError::UserNotFound),
            }
        } else {
            let emails: Vec<(String, String)> =
                sqlx::query_as("SELECT id, status FROM users WHERE email=?1 COLLATE NOCASE")
                    .bind(grant.identity.email)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(db)?;
            if emails.len() > 1 {
                return Err(AppError::DuplicateResource("邮箱".into()));
            }
            let id = if let Some((id, status)) = emails.into_iter().next() {
                if status == "disabled" {
                    return Err(AppError::AccountDisabled);
                }
                id
            } else {
                let username = unique_username(
                    &mut tx,
                    grant.identity.preferred_username,
                    grant.identity.username_suffix,
                )
                .await?;
                let now = Utc::now().timestamp_millis();
                sqlx::query(
                    r#"INSERT INTO users (id,username,email,password_hash,role,status,
                       must_change_password,max_devices,access_mode,created_at,updated_at)
                       VALUES (?1,?2,?3,?4,'user','active',0,1,'approval_required',?5,?5)"#,
                )
                .bind(grant.identity.new_user_id)
                .bind(username)
                .bind(grant.identity.email)
                .bind(grant.identity.password_hash)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
                grant.identity.new_user_id.to_string()
            };
            let other: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_identities WHERE provider='feishu' AND user_id=?1 AND subject!=?2)")
                .bind(&id).bind(grant.identity.subject).fetch_one(&mut *tx).await.map_err(db)?;
            if other {
                return Err(AppError::DuplicateResource(
                    "该账号已绑定其他飞书身份".into(),
                ));
            }
            sqlx::query(
                "INSERT INTO external_identities(provider,subject,user_id,created_at) VALUES('feishu',?1,?2,?3)",
            )
            .bind(grant.identity.subject)
            .bind(&id)
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            id
        };

        let prior: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT user_id,group_id,expires_at FROM access_grants WHERE approval_instance_code=?1",
        )
        .bind(grant.instance_code)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        if !prior.is_empty() {
            let prior_groups: std::collections::BTreeSet<&str> =
                prior.iter().map(|(_, group, _)| group.as_str()).collect();
            if prior_groups != group_ids || prior.iter().any(|(id, _, _)| id != &user_id) {
                return Err(AppError::Validation("审批实例授权内容发生冲突".into()));
            }
            sqlx::query("UPDATE access_grants SET expires_at=?2,reason=?3,updated_at=?4 WHERE approval_instance_code=?1 AND expires_at<?2")
                .bind(grant.instance_code).bind(grant.expires_at).bind(grant.reason)
                .bind(Utc::now().timestamp_millis()).execute(&mut *tx).await.map_err(db)?;
        } else {
            let now = Utc::now().timestamp_millis();
            for group_id in group_ids {
                sqlx::query(r#"INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,reason,created_at,updated_at)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?7)"#)
                    .bind(uuid::Uuid::now_v7().to_string()).bind(grant.instance_code).bind(&user_id)
                    .bind(group_id).bind(grant.expires_at).bind(grant.reason).bind(now)
                    .execute(&mut *tx).await.map_err(db)?;
            }
        }
        // Queue only a newly applied instance, in the same transaction as its grants.
        // Replayed callbacks and mail retries cannot create another notification.
        if prior.is_empty() {
            let groups: Vec<String> = sqlx::query_scalar("SELECT g.name FROM access_grants a JOIN user_groups g ON g.id=a.group_id WHERE a.approval_instance_code=?1 ORDER BY g.name")
                .bind(grant.instance_code).fetch_all(&mut *tx).await.map_err(db)?;
            let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id=?1")
                .bind(&user_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
            let expiry = chrono::DateTime::from_timestamp_millis(grant.expires_at)
                .ok_or_else(|| AppError::Validation("授权到期时间非法".into()))?
                .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                .format("%Y-%m-%d %H:%M:%S（北京时间）")
                .to_string();
            let body = serde_json::json!({
                "username": username,
                "applicant_email": grant.identity.email,
                "user_groups": groups.join("、"),
                "expires_at": expiry,
                "instance_code": grant.instance_code,
            })
            .to_string();
            sqlx::query("INSERT INTO approval_mail_outbox(instance_code,recipient,body,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(instance_code) DO NOTHING")
                .bind(grant.instance_code).bind(grant.identity.email).bind(body).bind(grant.expires_at)
                .execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(user_id)
    }
}

async fn unique_username(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    preferred: &str,
    suffix: &str,
) -> Result<String> {
    for candidate in std::iter::once(preferred.to_string())
        .chain(std::iter::once(format!("{preferred}-{suffix}")))
        .chain((2..10_000).map(|n| format!("{preferred}-{suffix}-{n}")))
    {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE username=?1")
            .bind(&candidate)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
        if count.0 == 0 {
            return Ok(candidate);
        }
    }
    Err(AppError::DuplicateResource("用户名".into()))
}

fn truncate(error: &str) -> String {
    error.chars().take(500).collect()
}

fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup() -> SqlitePool {
        let url = format!(
            "sqlite:file:approval_repo_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    fn grant<'a>(instance: &'a str, expires_at: i64) -> ApprovedGrant<'a> {
        ApprovedGrant {
            instance_code: instance,
            group_ids: vec!["g1".into()],
            expires_at,
            reason: "need access",
            identity: ApprovalIdentity {
                subject: "union-1",
                email: "alice@example.com",
                preferred_username: "alice",
                username_suffix: "12345678",
                new_user_id: "u-new",
                password_hash: "hash",
            },
        }
    }

    #[tokio::test]
    async fn approval_mail_is_atomic_deduplicated_and_retries_with_a_lease() {
        let pool = setup().await;
        let repo = SqliteAccessGrantRepository::new(pool.clone());
        assert!(repo.apply_approved(grant("invalid", 20_000)).await.is_err());
        assert!(repo.claim_mail().await.unwrap().is_none());
        sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES('g1','ops','',0,0),('g2','dev','',0,0)")
            .execute(&pool).await.unwrap();
        let mut approved = grant("mail-instance", 20_000);
        approved.group_ids = vec!["g1".into(), "g2".into(), "g1".into()];
        repo.apply_approved(approved.clone()).await.unwrap();
        repo.apply_approved(approved).await.unwrap();
        let first = repo.claim_mail().await.unwrap().unwrap();
        assert_eq!(first.recipient, "alice@example.com");
        assert!(first.body.contains("dev、ops"));
        assert!(first.body.contains("北京时间"));
        assert!(repo.claim_mail().await.unwrap().is_none());
        repo.finish_mail(&first, Some("SMTP unavailable"))
            .await
            .unwrap();
        assert!(repo.claim_mail().await.unwrap().is_none());
        let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM access_grants")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(grants, 2);
        sqlx::query("UPDATE approval_mail_outbox SET next_attempt_at=0")
            .execute(&pool)
            .await
            .unwrap();
        let second = repo.claim_mail().await.unwrap().unwrap();
        assert_eq!(second.attempts, 2);
        repo.finish_mail(&first, None).await.unwrap();
        let done: bool = sqlx::query_scalar("SELECT done FROM approval_mail_outbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!done, "stale claims cannot acknowledge another attempt");
        repo.finish_mail(&second, None).await.unwrap();
        assert!(repo.claim_mail().await.unwrap().is_none());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_mail_outbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn inbox_is_durable_idempotent_and_rejects_changed_replay() {
        let repo = SqliteAccessGrantRepository::new(setup().await);
        assert_eq!(
            repo.enqueue("e1", "i1", "h1", "{}").await.unwrap(),
            EnqueueResult::Inserted
        );
        assert_eq!(
            repo.enqueue("e1", "i1", "h1", "{}").await.unwrap(),
            EnqueueResult::Duplicate
        );
        assert_eq!(
            repo.enqueue("e1", "i1", "changed", "{}").await.unwrap(),
            EnqueueResult::Conflict
        );
        assert_eq!(
            repo.enqueue("e2", "i1", "h1", "{}").await.unwrap(),
            EnqueueResult::Duplicate
        );
        assert_eq!(
            repo.enqueue("e3", "i1", "changed", "{}").await.unwrap(),
            EnqueueResult::Conflict
        );
        let claimed = repo.claim_due().await.unwrap().unwrap();
        assert_eq!(claimed.event_id, "e1");
        repo.finish("e1").await.unwrap();
        assert!(repo.claim_due().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_creates_restricted_user_without_touching_manual_groups_and_never_shortens() {
        let pool = setup().await;
        sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES('g1','ops','10.0.0.0/8',0,0)")
            .execute(&pool).await.unwrap();
        let repo = SqliteAccessGrantRepository::new(pool.clone());
        assert_eq!(
            repo.apply_approved(grant("i1", 20_000)).await.unwrap(),
            "u-new"
        );
        let user: (String,) = sqlx::query_as("SELECT access_mode FROM users WHERE id='u-new'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(user.0, "approval_required");
        let manual: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM user_group_members WHERE user_id='u-new'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(manual.0, 0);
        repo.apply_approved(grant("i1", 10_000)).await.unwrap();
        let expiry: (i64,) = sqlx::query_as(
            "SELECT expires_at FROM access_grants WHERE approval_instance_code='i1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(expiry.0, 20_000);
    }

    #[tokio::test]
    async fn multiple_groups_are_atomic_idempotent_and_reject_changed_sets() {
        let pool = setup().await;
        for id in ["g1", "g2"] {
            sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES(?1,?1,'10.0.0.0/8',0,0)")
                .bind(id).execute(&pool).await.unwrap();
        }
        let repo = SqliteAccessGrantRepository::new(pool.clone());
        let mut invalid = grant("i1", 20_000);
        invalid.group_ids = vec!["g1".into(), "missing".into()];
        assert!(matches!(
            repo.apply_approved(invalid).await,
            Err(AppError::Validation(_))
        ));
        let users: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users.0, 0);
        for (groups, expires) in [
            (vec!["g1", "g2", "g1"], 20_000),
            (vec!["g2", "g1"], 10_000),
            (vec!["g1", "g2"], 30_000),
        ] {
            let mut approved = grant("i1", expires);
            approved.group_ids = groups.into_iter().map(str::to_string).collect();
            assert_eq!(repo.apply_approved(approved).await.unwrap(), "u-new");
        }
        assert!(matches!(
            repo.apply_approved(grant("i1", 40_000)).await,
            Err(AppError::Validation(_))
        ));
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT group_id,expires_at FROM access_grants ORDER BY group_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, vec![("g1".into(), 30_000), ("g2".into(), 30_000)]);
        let identities: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM external_identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(identities.0, 1);
    }

    #[tokio::test]
    async fn existing_email_remains_legacy_and_disabled_identity_is_rejected() {
        let pool = setup().await;
        sqlx::query("INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES('g1','ops','10.0.0.0/8',0,0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO users(id,username,email,password_hash,role,status,must_change_password,max_devices,created_at,updated_at) VALUES('legacy','legacy','alice@example.com','h','user','active',0,1,0,0)")
            .execute(&pool).await.unwrap();
        let repo = SqliteAccessGrantRepository::new(pool.clone());
        assert_eq!(
            repo.apply_approved(grant("i1", 20_000)).await.unwrap(),
            "legacy"
        );
        let mode: (String,) = sqlx::query_as("SELECT access_mode FROM users WHERE id='legacy'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.0, "legacy");
        sqlx::query("UPDATE users SET status='disabled' WHERE id='legacy'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            repo.apply_approved(grant("i2", 30_000)).await,
            Err(AppError::AccountDisabled)
        ));
    }
}
