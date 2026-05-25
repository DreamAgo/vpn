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
}
