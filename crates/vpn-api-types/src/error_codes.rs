//! 业务错误码常量。
//!
//! 分段约定（见 PRD §7 / Architecture §Error Codes）：
//! - `1xxx` 认证错误
//! - `2xxx` 权限错误
//! - `3xxx` 资源错误
//! - `4xxx` 限额/限速
//! - `5xxx` 系统错误
//!
//! 0 是保留的成功码。

// ===== 1xxx 认证错误 =====

/// 用户名或密码错误（不区分两种失败原因，防用户名枚举）。
pub const INVALID_CREDENTIALS: i32 = 1001;
/// Token 过期或无效。
pub const TOKEN_EXPIRED: i32 = 1002;
/// 账号已锁定（连续登录失败）。
pub const ACCOUNT_LOCKED: i32 = 1003;
/// 账号已禁用。
pub const ACCOUNT_DISABLED: i32 = 1004;
/// 密码强度不足。
pub const PASSWORD_TOO_WEAK: i32 = 1005;
/// 首次登录必须先修改密码。
pub const MUST_CHANGE_PASSWORD: i32 = 1006;
/// 缺失 Authorization Header。
pub const MISSING_AUTH: i32 = 1007;

// ===== 2xxx 权限错误 =====

/// 需要 admin 角色。
pub const REQUIRE_ADMIN: i32 = 2001;
/// 无权访问该资源（如尝试修改/删除自己的账号）。
pub const NO_ACCESS: i32 = 2002;

// ===== 3xxx 资源错误 =====

/// 用户不存在。
pub const USER_NOT_FOUND: i32 = 3001;
/// 节点不存在。
pub const PEER_NOT_FOUND: i32 = 3002;
/// 用户名/邮箱已存在。
pub const DUPLICATE_RESOURCE: i32 = 3003;
/// 系统未初始化（admin 未创建）。
pub const NOT_INITIALIZED: i32 = 3004;
/// 系统已初始化（first-time-setup 重复调用）。
pub const ALREADY_INITIALIZED: i32 = 3005;

// ===== 4xxx 限额/限速 =====

/// 请求过频（限速）。
pub const RATE_LIMITED: i32 = 4001;
/// 节点数超过限额。
pub const PEER_QUOTA_EXCEEDED: i32 = 4002;
/// IP 地址池耗尽。
pub const IP_POOL_EXHAUSTED: i32 = 4003;

// ===== 5xxx 系统错误 =====

/// 数据库错误（不暴露细节给客户端）。
pub const DATABASE_ERROR: i32 = 5001;
/// WireGuard 配置失败。
pub const WIREGUARD_ERROR: i32 = 5002;
/// 内部错误。
pub const INTERNAL_ERROR: i32 = 5003;
/// 配置错误（如缺少环境变量）。
pub const CONFIG_ERROR: i32 = 5004;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_in_correct_ranges() {
        // 1xxx 范围
        for code in [
            INVALID_CREDENTIALS,
            TOKEN_EXPIRED,
            ACCOUNT_LOCKED,
            ACCOUNT_DISABLED,
            PASSWORD_TOO_WEAK,
            MUST_CHANGE_PASSWORD,
            MISSING_AUTH,
        ] {
            assert!(
                (1000..2000).contains(&code),
                "auth code {} out of range",
                code
            );
        }
        // 2xxx 范围
        for code in [REQUIRE_ADMIN, NO_ACCESS] {
            assert!((2000..3000).contains(&code));
        }
        // 3xxx 范围
        for code in [
            USER_NOT_FOUND,
            PEER_NOT_FOUND,
            DUPLICATE_RESOURCE,
            NOT_INITIALIZED,
            ALREADY_INITIALIZED,
        ] {
            assert!((3000..4000).contains(&code));
        }
        // 4xxx 范围
        for code in [RATE_LIMITED, PEER_QUOTA_EXCEEDED, IP_POOL_EXHAUSTED] {
            assert!((4000..5000).contains(&code));
        }
        // 5xxx 范围
        for code in [
            DATABASE_ERROR,
            WIREGUARD_ERROR,
            INTERNAL_ERROR,
            CONFIG_ERROR,
        ] {
            assert!((5000..6000).contains(&code));
        }
    }
}
