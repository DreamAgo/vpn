//! vpn-cli 统一错误类型。
//!
//! 把底层（reqwest / vpn-platform / serde / io）错误以及服务端业务错误码
//! 收敛到一个枚举，便于 CLI 顶层统一打印与退出码映射。

use thiserror::Error;

/// vpn-cli 结果别名。
pub type CliResult<T> = std::result::Result<T, CliError>;

/// vpn-cli 顶层错误。
#[derive(Debug, Error)]
pub enum CliError {
    /// HTTP 传输层错误（连接失败、超时等）。
    #[error("网络请求失败: {0}")]
    Http(String),

    /// 服务端返回业务错误（非 0 code）。
    #[error("服务端错误 [code={code}]: {message}")]
    Api {
        /// 业务错误码（见 vpn_api_types::error_codes）。
        code: i32,
        /// 人类可读消息。
        message: String,
    },

    /// 响应体反序列化失败 / 信封缺少 data。
    #[error("响应解析失败: {0}")]
    Decode(String),

    /// 凭证存储 / TUN / daemon 等平台层错误。
    #[error("平台错误: {0}")]
    Platform(String),

    /// 未登录（缺少已保存的凭证）。
    #[error("尚未登录，请先执行 `vpn-cli login`")]
    NotLoggedIn,

    /// IPC 通信错误（与后台 daemon 无法通信）。
    #[error("无法与后台 daemon 通信: {0}（daemon 可能未运行）")]
    Ipc(String),

    /// 输入 / 配置错误。
    #[error("{0}")]
    Invalid(String),

    /// 其它内部错误。
    #[error("{0}")]
    Other(String),
}

impl CliError {
    /// 是否为「access token 过期 / 失效」错误（用于触发刷新重试）。
    pub fn is_token_expired(&self) -> bool {
        matches!(
            self,
            CliError::Api {
                code: vpn_api_types::error_codes::TOKEN_EXPIRED,
                ..
            }
        )
    }

    /// 是否为「服务端已无此 peer」（被管理员彻底删除）错误。
    ///
    /// 心跳遇此应视为致命：拆隧道并提示重新登录，而非当瞬时错误无限重连。
    pub fn is_peer_gone(&self) -> bool {
        matches!(
            self,
            CliError::Api {
                code: vpn_api_types::error_codes::PEER_NOT_FOUND,
                ..
            }
        )
    }

    /// 适合持久化日志的错误文本：敏感标记出现时整段遮蔽，并限制长度。
    pub fn safe_diagnostic(&self) -> String {
        match self {
            // 服务端 message 属于不可信任的任意文本；日志只保留固定错误码。
            CliError::Api { code, .. } => format!("服务端错误 [code={code}]"),
            other => redact_sensitive(&other.to_string()),
        }
    }
}

/// 对可能来自 panic、服务端响应或第三方库的文本做保守脱敏。
pub fn redact_sensitive(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    const SENSITIVE: &[&str] = &[
        "password",
        "token",
        "authorization",
        "bearer",
        "private_key",
        "private key",
        "refresh_token",
        "access_token",
        "client_secret",
        "api_key",
        "set-cookie",
        "cookie:",
        "credential",
        "secret",
    ];
    if SENSITIVE.iter().any(|needle| lower.contains(needle))
        || contains_url_userinfo(value)
        || contains_jwt_like_value(value)
    {
        return "[REDACTED sensitive diagnostic]".to_string();
    }
    value.chars().take(1024).collect()
}

fn contains_url_userinfo(value: &str) -> bool {
    let Some((_, after_scheme)) = value.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.contains('@')
}

fn contains_jwt_like_value(value: &str) -> bool {
    value
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';'))
        .any(|word| {
            let segments: Vec<_> = word.split('.').collect();
            segments.len() == 3
                && segments.iter().all(|segment| {
                    segment.len() >= 8
                        && segment
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                })
        })
}

impl From<reqwest::Error> for CliError {
    fn from(e: reqwest::Error) -> Self {
        CliError::Http(e.to_string())
    }
}

impl From<vpn_platform::PlatformError> for CliError {
    fn from(e: vpn_platform::PlatformError) -> Self {
        CliError::Platform(e.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Decode(e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_api_types::error_codes;

    #[test]
    fn token_expired_detection() {
        let e = CliError::Api {
            code: error_codes::TOKEN_EXPIRED,
            message: "x".to_string(),
        };
        assert!(e.is_token_expired());

        let e2 = CliError::Api {
            code: error_codes::INVALID_CREDENTIALS,
            message: "x".to_string(),
        };
        assert!(!e2.is_token_expired());

        assert!(!CliError::NotLoggedIn.is_token_expired());
    }

    #[test]
    fn diagnostic_text_is_redacted_and_bounded() {
        assert_eq!(
            redact_sensitive("Authorization: Bearer secret"),
            "[REDACTED sensitive diagnostic]"
        );
        assert_eq!(redact_sensitive("ordinary failure"), "ordinary failure");
        assert_eq!(
            redact_sensitive("https://user:pass@example.com/api"),
            "[REDACTED sensitive diagnostic]"
        );
        assert_eq!(
            redact_sensitive("aaaaaaaa.bbbbbbbb.cccccccc"),
            "[REDACTED sensitive diagnostic]"
        );
        assert_eq!(redact_sensitive(&"x".repeat(2048)).len(), 1024);

        let api_error = CliError::Api {
            code: 4001,
            message: "bare-sensitive-value".to_string(),
        };
        assert_eq!(api_error.safe_diagnostic(), "服务端错误 [code=4001]");
    }
}
