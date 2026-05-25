//! 业务错误类型与统一 Result 别名。
//!
//! 所有 service / repository 层方法返回 `Result<T>`。
//! HTTP 层会把 [`AppError`] 转换为 [`vpn_api_types::ApiResponse`]（在 vpn-server crate 实现 `IntoResponse`）。

use thiserror::Error;
use vpn_api_types::error_codes;

/// 统一业务错误。
///
/// 每个变体都对应 [`vpn_api_types::error_codes`] 中的某个码。
/// 通过 [`AppError::code`] 方法可得到业务码。
#[derive(Debug, Error)]
pub enum AppError {
    // ===== 1xxx 认证 =====
    #[error("用户名或密码错误")]
    InvalidCredentials,

    #[error("Token 过期或无效")]
    TokenExpired,

    #[error("账号已锁定，请稍后再试")]
    AccountLocked,

    #[error("账号已禁用，请联系管理员")]
    AccountDisabled,

    #[error("密码强度不足：{0}")]
    PasswordTooWeak(String),

    #[error("首次登录请先修改密码")]
    MustChangePassword,

    #[error("缺失认证信息")]
    MissingAuth,

    // ===== 2xxx 权限 =====
    #[error("需要 admin 角色")]
    RequireAdmin,

    #[error("无权访问该资源")]
    NoAccess,

    #[error("{0}")]
    NoAccessReason(String),

    // ===== 3xxx 资源 =====
    #[error("用户不存在")]
    UserNotFound,

    #[error("节点不存在")]
    PeerNotFound,

    #[error("{0} 已存在")]
    DuplicateResource(String),

    #[error("系统未初始化")]
    NotInitialized,

    #[error("系统已初始化")]
    AlreadyInitialized,

    // ===== 4xxx 限额 =====
    #[error("请求过频，请稍后再试")]
    RateLimited,

    #[error("节点数已达上限")]
    PeerQuotaExceeded,

    #[error("IP 地址池已耗尽")]
    IpPoolExhausted,

    // ===== 5xxx 系统 =====
    #[error("数据库错误")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("WireGuard 配置失败：{0}")]
    WireGuard(String),

    #[error("内部错误：{0}")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("配置错误：{0}")]
    Config(String),
}

impl AppError {
    /// 取得业务错误码（对应 [`vpn_api_types::error_codes`]）。
    pub fn code(&self) -> i32 {
        match self {
            AppError::InvalidCredentials => error_codes::INVALID_CREDENTIALS,
            AppError::TokenExpired => error_codes::TOKEN_EXPIRED,
            AppError::AccountLocked => error_codes::ACCOUNT_LOCKED,
            AppError::AccountDisabled => error_codes::ACCOUNT_DISABLED,
            AppError::PasswordTooWeak(_) => error_codes::PASSWORD_TOO_WEAK,
            AppError::MustChangePassword => error_codes::MUST_CHANGE_PASSWORD,
            AppError::MissingAuth => error_codes::MISSING_AUTH,
            AppError::RequireAdmin => error_codes::REQUIRE_ADMIN,
            AppError::NoAccess => error_codes::NO_ACCESS,
            AppError::NoAccessReason(_) => error_codes::NO_ACCESS,
            AppError::UserNotFound => error_codes::USER_NOT_FOUND,
            AppError::PeerNotFound => error_codes::PEER_NOT_FOUND,
            AppError::DuplicateResource(_) => error_codes::DUPLICATE_RESOURCE,
            AppError::NotInitialized => error_codes::NOT_INITIALIZED,
            AppError::AlreadyInitialized => error_codes::ALREADY_INITIALIZED,
            AppError::RateLimited => error_codes::RATE_LIMITED,
            AppError::PeerQuotaExceeded => error_codes::PEER_QUOTA_EXCEEDED,
            AppError::IpPoolExhausted => error_codes::IP_POOL_EXHAUSTED,
            AppError::Database(_) => error_codes::DATABASE_ERROR,
            AppError::WireGuard(_) => error_codes::WIREGUARD_ERROR,
            AppError::Internal(_) => error_codes::INTERNAL_ERROR,
            AppError::Config(_) => error_codes::CONFIG_ERROR,
        }
    }

    /// 是否为客户端错误（4xx HTTP），相对于服务端错误（5xx）。
    pub fn is_client_error(&self) -> bool {
        !matches!(
            self,
            AppError::Database(_)
                | AppError::WireGuard(_)
                | AppError::Internal(_)
                | AppError::Config(_)
        )
    }
}

/// 包装 service / repository 层的 Result 别名。
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_expected_business_codes() {
        assert_eq!(AppError::InvalidCredentials.code(), 1001);
        assert_eq!(AppError::TokenExpired.code(), 1002);
        assert_eq!(AppError::RequireAdmin.code(), 2001);
        assert_eq!(AppError::UserNotFound.code(), 3001);
        assert_eq!(AppError::RateLimited.code(), 4001);
        assert_eq!(AppError::WireGuard("test".to_string()).code(), 5002);
    }

    #[test]
    fn client_vs_server_error_classification() {
        assert!(AppError::InvalidCredentials.is_client_error());
        assert!(AppError::UserNotFound.is_client_error());
        assert!(!AppError::WireGuard("oops".to_string()).is_client_error());
        assert!(!AppError::Config("missing env".to_string()).is_client_error());
    }

    #[test]
    fn display_messages_in_chinese() {
        assert_eq!(AppError::InvalidCredentials.to_string(), "用户名或密码错误");
        assert_eq!(AppError::RequireAdmin.to_string(), "需要 admin 角色");
    }
}
