//! vpn-platform 自有错误类型。
//!
//! 刻意不依赖 `vpn-core::AppError`，避免平台抽象层与业务核心强耦合；
//! 下游（如 vpn-cli）可在需要时把 [`PlatformError`] 映射到自己的错误体系。

use std::io;

/// 平台抽象层统一结果别名。
pub type Result<T> = std::result::Result<T, PlatformError>;

/// 跨平台抽象层错误。
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// 底层 IO 错误（设备读写、文件读写、子进程调用等）。
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// 输入参数非法（如 CIDR 格式错误）。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// 当前平台不支持该操作（编译期门控之外的运行期兜底）。
    #[error("operation not supported on this platform: {0}")]
    Unsupported(String),

    /// TUN 设备相关错误。
    #[error("tun device error: {0}")]
    Tun(String),

    /// 凭证存储相关错误（keyring / 文件后端）。
    #[error("credential store error: {0}")]
    Credential(String),

    /// 凭证文件加解密错误。
    #[error("credential crypto error: {0}")]
    Crypto(String),

    /// Daemon / 服务管理相关错误。
    #[error("daemon runtime error: {0}")]
    Daemon(String),

    /// 系统命令执行失败（携带退出码与 stderr 摘要）。
    #[error("command `{command}` failed: {message}")]
    Command {
        /// 执行的命令名（如 `systemctl` / `launchctl` / `sc`）。
        command: String,
        /// 失败描述（含退出码与 stderr）。
        message: String,
    },
}

impl PlatformError {
    /// 构造一个 [`PlatformError::Command`]。
    pub fn command(command: impl Into<String>, message: impl Into<String>) -> Self {
        PlatformError::Command {
            command: command.into(),
            message: message.into(),
        }
    }
}
