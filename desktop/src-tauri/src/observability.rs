//! 桌面客户端本地可观测性：滚动文件日志、保留清理、panic 记录和诊断元数据。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

const LOG_RETENTION_DAYS: u64 = 7;
const LOG_RETENTION_FILES: usize = 7;
const LOG_FILE_PREFIX: &str = "vpn-desktop.log";
const LOG_SNAPSHOT_MAX_LINES: usize = 500;
const LOG_SNAPSHOT_MAX_BYTES: usize = 256 * 1024;

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

/// 从固定日志目录读取的近期脱敏日志快照。
#[derive(Debug, Clone, Serialize)]
pub struct LogSnapshot {
    /// 按时间正序排列的近期日志文本。
    pub content: String,
    /// `content` 中的日志行数。
    pub line_count: usize,
    /// 是否因文件轮转、行数或字节上限省略了更早内容。
    pub truncated: bool,
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

/// 读取固定本地日志目录中的近期内容；不接受调用方路径或大小参数。
pub fn recent_logs() -> Result<LogSnapshot, String> {
    let dir = log_dir().ok_or_else(|| "无法定位本地日志目录".to_string())?;
    read_recent_logs_from(&dir, LOG_SNAPSHOT_MAX_LINES, LOG_SNAPSHOT_MAX_BYTES)
}

fn read_recent_logs_from(
    dir: &Path,
    max_lines: usize,
    max_bytes: usize,
) -> Result<LogSnapshot, String> {
    if max_lines == 0 || max_bytes == 0 {
        return Ok(LogSnapshot {
            content: String::new(),
            line_count: 0,
            truncated: false,
        });
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogSnapshot {
                content: String::new(),
                line_count: 0,
                truncated: false,
            });
        }
        Err(error) => return Err(format!("读取日志目录失败: {error}")),
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("枚举日志文件失败: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取日志文件类型失败: {error}"))?;
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_desktop_log_name(name) {
            continue;
        }
        files.push((name.to_string(), entry.path()));
    }
    // tracing-appender 的日期后缀是 ISO 日期，按文件名即可得到稳定的时间顺序；
    // 无日期的兼容文件视为当前活动文件，排在最后。
    files.sort_by(|a, b| log_name_sort_key(&a.0).cmp(&log_name_sort_key(&b.0)));

    let mut collected = Vec::new();
    let mut truncated = false;
    let mut partial_prefix = false;
    for (_, path) in files.iter().rev() {
        if collected.len() >= max_bytes {
            truncated = true;
            break;
        }
        let needs_separator = !collected.is_empty();
        let remaining = max_bytes - collected.len();
        if needs_separator && remaining <= 1 {
            truncated = true;
            break;
        }
        let read_budget = remaining - usize::from(needs_separator);
        let mut file = open_log_file(path)?;
        let length = file
            .metadata()
            .map_err(|error| format!("读取日志文件信息失败: {error}"))?
            .len();
        let take = read_budget.min(length.try_into().unwrap_or(usize::MAX));
        let start = length.saturating_sub(take as u64);
        let mut this_partial_prefix = false;
        if start > 0 {
            truncated = true;
            file.seek(SeekFrom::Start(start - 1))
                .map_err(|error| format!("定位日志文件失败: {error}"))?;
            let mut previous = [0_u8; 1];
            if file
                .read(&mut previous)
                .map_err(|error| format!("读取日志文件失败: {error}"))?
                == 1
            {
                this_partial_prefix = previous[0] != b'\n';
            }
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|error| format!("定位日志文件失败: {error}"))?;
        let mut part = Vec::with_capacity(take);
        file.take(take as u64)
            .read_to_end(&mut part)
            .map_err(|error| format!("读取日志文件失败: {error}"))?;
        if part.is_empty() {
            continue;
        }
        partial_prefix = this_partial_prefix;
        if needs_separator && !part.ends_with(b"\n") {
            part.push(b'\n');
        }
        part.extend_from_slice(&collected);
        collected = part;
    }
    let decoded = String::from_utf8_lossy(&collected);
    let all_lines: Vec<&str> = decoded.lines().collect();
    let complete_from = usize::from(partial_prefix).min(all_lines.len());
    let first = complete_from.max(all_lines.len().saturating_sub(max_lines));
    if first > 0 {
        truncated = true;
    }
    let mut redact_continuation = false;
    let mut safe_lines: Vec<String> = all_lines[first..]
        .iter()
        .map(|line| {
            let redacted = vpn_cli::error::redact_sensitive(line);
            let contains_marker = redacted != **line;
            let output = if redact_continuation || contains_marker {
                "[REDACTED sensitive diagnostic]".to_string()
            } else {
                redacted
            };
            // 保守遮蔽敏感字段后的续行，覆盖 `Authorization:\n<value>` 一类格式。
            redact_continuation = contains_marker && sensitive_value_may_follow(line);
            output
        })
        .collect();

    let mut content = safe_lines.join("\n");
    while content.len() > max_bytes && !safe_lines.is_empty() {
        truncated = true;
        safe_lines.remove(0);
        content = safe_lines.join("\n");
    }
    Ok(LogSnapshot {
        line_count: safe_lines.len(),
        content,
        truncated,
    })
}

fn sensitive_value_may_follow(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.ends_with(':') || trimmed.ends_with('=')
}

fn is_desktop_log_name(name: &str) -> bool {
    if name == LOG_FILE_PREFIX {
        return true;
    }
    let Some(date) = name.strip_prefix(&format!("{LOG_FILE_PREFIX}.")) else {
        return false;
    };
    date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7) && byte == b'-'
                || !matches!(index, 4 | 7) && byte.is_ascii_digit()
        })
}

fn log_name_sort_key(name: &str) -> (u8, &str) {
    if name == LOG_FILE_PREFIX {
        (1, name)
    } else {
        (0, name)
    }
}

fn open_log_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options
            .open(path)
            .map_err(|error| format!("安全打开日志文件失败: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("读取日志文件信息失败: {error}"))?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err("拒绝读取非普通文件或硬链接日志".to_string());
        }
        return Ok(file);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options
            .open(path)
            .map_err(|error| format!("安全打开日志文件失败: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("读取日志文件信息失败: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("拒绝读取非普通文件或重解析点日志".to_string());
        }
        return Ok(file);
    }

    #[allow(unreachable_code)]
    options
        .open(path)
        .map_err(|error| format!("打开日志文件失败: {error}"))
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
            .is_some_and(is_desktop_log_name);
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
        let old_log = dir.join("vpn-desktop.log.2026-07-01");
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

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "vpn-desktop-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn recent_logs_reads_rotations_in_chronological_order_and_redacts() {
        let dir = test_dir("tail-order");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("vpn-desktop.log.2026-07-17"),
            b"first\nBearer secret\n",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        fs::write(dir.join("vpn-desktop.log.2026-07-18"), b"third\n").unwrap();

        let snapshot = read_recent_logs_from(&dir, 500, 256 * 1024).unwrap();
        assert_eq!(
            snapshot.content,
            "first\n[REDACTED sensitive diagnostic]\nthird"
        );
        assert_eq!(snapshot.line_count, 3);
        assert!(!snapshot.truncated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_logs_separates_files_and_redacts_sensitive_continuations() {
        let dir = test_dir("tail-boundaries");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("vpn-desktop.log.2026-07-17"),
            b"first without newline",
        )
        .unwrap();
        fs::write(
            dir.join("vpn-desktop.log.2026-07-18"),
            b"Authorization:\nraw-value\nlast\n",
        )
        .unwrap();

        let snapshot = read_recent_logs_from(&dir, 500, 1024).unwrap();
        assert_eq!(
            snapshot.content,
            "first without newline\n[REDACTED sensitive diagnostic]\n[REDACTED sensitive diagnostic]\nlast"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_logs_enforces_line_and_byte_limits() {
        let dir = test_dir("tail-limits");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE_PREFIX), b"one\ntwo\nthree\nfour\n").unwrap();

        let line_limited = read_recent_logs_from(&dir, 2, 1024).unwrap();
        assert_eq!(line_limited.content, "three\nfour");
        assert!(line_limited.truncated);

        let byte_limited = read_recent_logs_from(&dir, 500, 9).unwrap();
        assert!(byte_limited.content.len() <= 9);
        assert!(byte_limited.truncated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_logs_ignores_unrelated_entries_and_symlinks() {
        let dir = test_dir("tail-filter");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE_PREFIX), b"kept\n").unwrap();
        fs::write(dir.join("unrelated.txt"), b"ignored\n").unwrap();
        fs::write(dir.join("vpn-desktop.log.injected"), b"injected\n").unwrap();
        fs::create_dir(dir.join("vpn-desktop.log.directory")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.join(LOG_FILE_PREFIX),
            dir.join("vpn-desktop.log.2026-07-18"),
        )
        .unwrap();

        let snapshot = read_recent_logs_from(&dir, 500, 1024).unwrap();
        assert_eq!(snapshot.content, "kept");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_logs_decodes_invalid_utf8_lossily() {
        let dir = test_dir("tail-utf8");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE_PREFIX), [b'o', b'k', b' ', 0xff, b'\n']).unwrap();

        let snapshot = read_recent_logs_from(&dir, 500, 1024).unwrap();
        assert_eq!(snapshot.content, "ok �");
        fs::remove_dir_all(dir).unwrap();
    }
}
