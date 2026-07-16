//! 桌面客户端本地可观测性：滚动文件日志、保留清理、panic 记录和诊断元数据。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

const LOG_RETENTION_DAYS: u64 = 7;
const LOG_RETENTION_FILES: usize = 7;
const LOG_FILE_PREFIX: &str = "vpn-desktop.log";

static DIAGNOSTICS: OnceLock<DiagnosticsInfo> = OnceLock::new();

/// 不含凭证的本地诊断元数据，可安全复制给运维人员。
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsInfo {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub log_dir: Option<String>,
    pub log_error: Option<String>,
}

/// 在 Tauri 初始化前启动文件日志。任何失败都降级，不阻止 GUI 启动。
pub fn init() -> DiagnosticsInfo {
    let mut info = DiagnosticsInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        log_dir: None,
        log_error: None,
    };

    match log_dir() {
        Some(dir) => match prepare_log_dir(&dir) {
            Ok(()) => {
                let cleanup_failures =
                    cleanup_old_logs(&dir, SystemTime::now(), LOG_RETENTION_DAYS);
                match build_appender(&dir) {
                    Ok(appender) => {
                        let filter = safe_env_filter();
                        match tracing_subscriber::fmt()
                            .with_env_filter(filter)
                            .with_ansi(false)
                            .with_writer(appender)
                            .try_init()
                        {
                            Ok(()) => {
                                info.log_dir = Some(dir.display().to_string());
                                if cleanup_failures > 0 {
                                    info.log_error =
                                        Some(format!("{cleanup_failures} 个过期日志清理失败"));
                                }
                            }
                            Err(e) => {
                                info.log_error = Some(format!("初始化日志订阅失败: {e}"));
                            }
                        }
                    }
                    Err(e) => info.log_error = Some(format!("创建日志文件失败: {e}")),
                }
            }
            Err(e) => info.log_error = Some(format!("准备日志目录失败: {e}")),
        },
        None => info.log_error = Some("无法定位本地数据目录".to_string()),
    }

    if let Some(error) = &info.log_error {
        eprintln!("vpn-desktop: 本地文件日志不可用: {error}");
    }
    let _ = DIAGNOSTICS.set(info.clone());
    install_panic_hook();
    tracing::info!(
        version = %info.app_version,
        os = %info.os,
        arch = %info.arch,
        pid = std::process::id(),
        log_dir = ?info.log_dir,
        log_error = ?info.log_error,
        "桌面客户端启动"
    );
    info
}

fn build_appender(dir: &Path) -> Result<RollingFileAppender, String> {
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(LOG_RETENTION_FILES)
        .build(dir)
        .map_err(|error| error.to_string())
}

/// 返回已初始化的安全诊断信息。
pub fn diagnostics() -> DiagnosticsInfo {
    DIAGNOSTICS
        .get()
        .cloned()
        .unwrap_or_else(|| DiagnosticsInfo {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            log_dir: None,
            log_error: Some("日志尚未初始化".to_string()),
        })
}

fn log_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|base| base.join("vpn-cli").join("logs"))
}

fn prepare_log_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn cleanup_old_logs(dir: &Path, now: SystemTime, retention_days: u64) -> usize {
    let max_age = Duration::from_secs(retention_days.saturating_mul(24 * 60 * 60));
    let Ok(entries) = fs::read_dir(dir) else {
        return 1;
    };
    let mut failures = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            failures += 1;
            continue;
        };
        let path = entry.path();
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(LOG_FILE_PREFIX));
        if !is_log {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if now.duration_since(modified).is_ok_and(|age| age > max_age)
            && fs::remove_file(path).is_err()
        {
            failures += 1;
        }
    }
    failures
}

/// 只允许本项目目标进入持久化文件，防止 `RUST_LOG=trace` 打开 HTTP 依赖的敏感事件。
fn safe_env_filter() -> EnvFilter {
    let level = std::env::var("RUST_LOG")
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| {
            matches!(
                value.as_str(),
                "off" | "error" | "warn" | "info" | "debug" | "trace"
            )
        })
        .unwrap_or_else(|| "info".to_string());
    EnvFilter::new(format!(
        "off,vpn_desktop_lib={level},vpn_cli={level},vpn_platform={level},vpn_wireguard={level}"
    ))
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let thread = std::thread::current();
        let raw_message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str));
        let message = raw_message
            .map(redact_panic_message)
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let original_is_safe = raw_message.is_none_or(|raw| raw == message);
        let location = panic
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            %location,
            %message,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "Rust panic"
        );
        // 默认 hook 会重放原始 payload；仅在文本未被脱敏时调用，避免秘密流入 stderr。
        if original_is_safe {
            previous(panic);
        }
    }));
}

fn redact_panic_message(message: &str) -> String {
    vpn_cli::error::redact_sensitive(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_messages_with_secret_markers_are_redacted() {
        assert_eq!(
            redact_panic_message("Bearer top-secret token"),
            "[REDACTED sensitive diagnostic]"
        );
        assert_eq!(redact_panic_message("ordinary failure"), "ordinary failure");
    }

    #[test]
    fn cleanup_only_removes_expired_desktop_logs() {
        let dir = std::env::temp_dir().join(format!(
            "vpn-desktop-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let old_log = dir.join("vpn-desktop.log.old");
        let unrelated = dir.join("keep.txt");
        fs::write(&old_log, b"old").unwrap();
        fs::write(&unrelated, b"keep").unwrap();

        let failures = cleanup_old_logs(
            &dir,
            SystemTime::now() + Duration::from_secs(8 * 24 * 60 * 60),
            7,
        );

        assert_eq!(failures, 0);
        assert!(!old_log.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn appender_creation_failure_is_returned_without_panic() {
        let path = std::env::temp_dir().join(format!(
            "vpn-desktop-file-instead-of-dir-{}",
            std::process::id()
        ));
        fs::write(&path, b"not a directory").unwrap();
        assert!(build_appender(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
