//! 认证业务服务（登录 / 注册 admin / 改密 / 注销）。

use chrono::{Duration, Utc};
use std::sync::Arc;
use uuid::Uuid;

use vpn_core::{
    service::{PasswordHasher, TokenIssuer},
    AppError, Result,
};

use crate::{
    ratelimit::LoginAttempts,
    repositories::{
        session_repo_sqlite::SqliteSessionRepository,
        user_repo_sqlite::{SqliteUserRepository, UserRow},
    },
    services::token_issuer::{hash_refresh_token, ACCESS_TOKEN_TTL_SECS, REFRESH_TOKEN_TTL_SECS},
};

#[derive(Clone)]
pub struct AuthService {
    pub user_repo: SqliteUserRepository,
    pub session_repo: SqliteSessionRepository,
    pub hasher: Arc<dyn PasswordHasher>,
    pub issuer: Arc<dyn TokenIssuer>,
    pub login_attempts: LoginAttempts,
}

pub struct LoginOutcome {
    pub user: UserRow,
    pub access_token: String,
    pub refresh_token: String,
}

impl AuthService {
    /// 创建首位 admin（first-time-setup）。
    ///
    /// 仅当 users 表无 admin 时可调用。
    pub async fn first_time_setup(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<LoginOutcome> {
        if self.user_repo.count_admins().await? > 0 {
            return Err(AppError::AlreadyInitialized);
        }
        Self::validate_password(password)?;
        let hash = self.hasher.hash(password)?;
        let user_id = Uuid::now_v7().to_string();
        let user = self
            .user_repo
            .insert(&user_id, username, email, &hash, "admin", false)
            .await?;
        let access = self.issuer.issue_access(&user.id, &user.role).await?;
        let refresh = self.issuer.issue_refresh(&user.id).await?;
        self.persist_session(&user.id, &refresh, None, None).await?;
        Ok(LoginOutcome {
            user,
            access_token: access,
            refresh_token: refresh,
        })
    }

    /// 账号密码登录。
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> Result<LoginOutcome> {
        let now = Utc::now();
        if self.login_attempts.is_locked(username, now).await {
            return Err(AppError::AccountLocked);
        }

        let user = match self.user_repo.find_by_username(username).await? {
            Some(u) => u,
            None => {
                self.login_attempts.record_failure(username, now).await;
                // 不区分用户名/密码错误（防用户名枚举）
                return Err(AppError::InvalidCredentials);
            }
        };

        if user.status == "disabled" {
            return Err(AppError::AccountDisabled);
        }

        if !self.hasher.verify(password, &user.password_hash)? {
            self.login_attempts.record_failure(username, now).await;
            return Err(AppError::InvalidCredentials);
        }

        // 登录成功
        self.login_attempts.reset(username).await;
        self.user_repo.update_last_login(&user.id).await?;
        let access = self.issuer.issue_access(&user.id, &user.role).await?;
        let refresh = self.issuer.issue_refresh(&user.id).await?;
        self.persist_session(&user.id, &refresh, ip, ua).await?;
        Ok(LoginOutcome {
            user,
            access_token: access,
            refresh_token: refresh,
        })
    }

    /// 用 Refresh Token 换取新 Access Token。
    pub async fn refresh(&self, refresh_token: &str) -> Result<String> {
        let hash = hash_refresh_token(refresh_token);
        let now_ms = Utc::now().timestamp_millis();
        let session = self
            .session_repo
            .find_active_by_token_hash(&hash, now_ms)
            .await?
            .ok_or(AppError::TokenExpired)?;
        let user = self
            .user_repo
            .find_by_id(&session.user_id)
            .await?
            .ok_or(AppError::TokenExpired)?;
        if user.status == "disabled" {
            return Err(AppError::AccountDisabled);
        }
        self.issuer.issue_access(&user.id, &user.role).await
    }

    /// 主动注销（撤销 Refresh Token）。
    pub async fn logout(&self, refresh_token: &str) -> Result<()> {
        let hash = hash_refresh_token(refresh_token);
        self.session_repo.revoke(&hash).await
    }

    /// 修改密码：验证旧密码 → 校验新密码强度 → 更新哈希 → 撤销所有 session。
    pub async fn change_password(
        &self,
        user_id: &str,
        old_password: &str,
        new_password: &str,
    ) -> Result<()> {
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        if !self.hasher.verify(old_password, &user.password_hash)? {
            return Err(AppError::InvalidCredentials);
        }
        Self::validate_password(new_password)?;
        let new_hash = self.hasher.hash(new_password)?;
        self.user_repo
            .update_password(user_id, &new_hash, true)
            .await?;
        self.session_repo.revoke_all_for_user(user_id).await?;
        Ok(())
    }

    /// 密码强度校验：≥ 8 位 + 含字母 + 含数字。
    pub fn validate_password(password: &str) -> Result<()> {
        if password.len() < 8 {
            return Err(AppError::PasswordTooWeak("至少 8 位".to_string()));
        }
        let has_letter = password.chars().any(|c| c.is_alphabetic());
        let has_digit = password.chars().any(|c| c.is_ascii_digit());
        if !has_letter || !has_digit {
            return Err(AppError::PasswordTooWeak("需含字母与数字".to_string()));
        }
        Ok(())
    }

    async fn persist_session(
        &self,
        user_id: &str,
        refresh_token: &str,
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> Result<()> {
        let hash = hash_refresh_token(refresh_token);
        let id = Uuid::now_v7().to_string();
        let expires_at =
            (Utc::now() + Duration::seconds(REFRESH_TOKEN_TTL_SECS)).timestamp_millis();
        self.session_repo
            .create(&id, user_id, &hash, ip, ua, expires_at)
            .await
    }

    /// Access Token TTL（用于响应中告知客户端何时该刷新）。
    pub fn access_ttl_secs() -> i64 {
        ACCESS_TOKEN_TTL_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_password_rejects_short() {
        assert!(matches!(
            AuthService::validate_password("abc12"),
            Err(AppError::PasswordTooWeak(_))
        ));
    }

    #[test]
    fn validate_password_rejects_no_letter() {
        assert!(matches!(
            AuthService::validate_password("12345678"),
            Err(AppError::PasswordTooWeak(_))
        ));
    }

    #[test]
    fn validate_password_rejects_no_digit() {
        assert!(matches!(
            AuthService::validate_password("abcdefgh"),
            Err(AppError::PasswordTooWeak(_))
        ));
    }

    #[test]
    fn validate_password_accepts_strong() {
        assert!(AuthService::validate_password("abc12345").is_ok());
        assert!(AuthService::validate_password("MyP@ssw0rd").is_ok());
    }
}
