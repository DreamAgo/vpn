//! Linux unattended updates. The timer runs a separate, known-working copy of
//! vpn-cli, so a broken replacement cannot disable recovery. No VPN credentials
//! are needed to fetch the public HTTPS manifest/packages.
use crate::ipc::{self, ConnState, IpcRequest, IpcResponse};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Url;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};

const ROOT: &str = "/var/lib/vpn-cli-update";
const CONFIG: &str = "/etc/vpn-cli-update.json";
const RUNNER: &str = "/usr/local/libexec/vpn-cli-updater";
const UNIT: &str = "/etc/systemd/system/vpn-cli.service";
const UPDATE_UNIT: &str = "/etc/systemd/system/vpn-cli-update.service";
const TIMER: &str = "/etc/systemd/system/vpn-cli-update.timer";
const MAX_BINARY: usize = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Config {
    server: String,
    binary: PathBuf,
}
#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: BTreeMap<String, Package>,
}
#[derive(Deserialize)]
struct Package {
    url: String,
    signature: String,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    version: String,
    was_active: bool,
}

fn require_root() -> Result<()> {
    ensure!(cfg!(target_os = "linux"), "仅支持 Linux systemd 系统服务");
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "请使用 sudo 执行系统服务更新管理"
    );
    Ok(())
}

// Reject writable/symlinked paths before persisting a root updater command.
fn trusted_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "必须使用绝对路径");
    for item in path.ancestors() {
        let meta =
            fs::symlink_metadata(item).with_context(|| format!("检查 {}", item.display()))?;
        ensure!(
            !meta.file_type().is_symlink() && meta.uid() == 0 && meta.mode() & 0o022 == 0,
            "{} 必须由 root 所有，且不能为符号链接或允许其他用户写入",
            item.display()
        );
    }
    Ok(())
}

fn secure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    trusted_path(path)?;
    Ok(())
}

fn lock(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("lock"))?;
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "另一个安装或升级任务正在运行"
    );
    Ok(file)
}

/// Same-directory rename with fsync: executable replacement and journal writes
/// must survive power loss. A stale temporary is harmless and removed on retry.
fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let temp = path.with_extension("update-tmp");
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    File::open(path.parent().context("缺少父目录")?)?.sync_all()?;
    Ok(())
}

fn remove_synced(path: &Path) -> Result<()> {
    fs::remove_file(path)?;
    File::open(path.parent().context("缺少父目录")?)?.sync_all()?;
    Ok(())
}

fn endpoint(server: &str) -> Result<Url> {
    let mut url = secure_url(server)?;
    ensure!(url.path() == "/", "更新服务地址只接受 HTTPS 域名和端口");
    url.set_path("/updates/latest.json");
    Ok(url)
}
fn secure_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "更新地址必须为 HTTPS，不能包含凭证、查询参数或片段"
    );
    Ok(url)
}
fn package_url(manifest: &Url, value: &str) -> Result<Url> {
    let url = secure_url(value)?;
    ensure!(
        url.origin() == manifest.origin() && url.path().starts_with("/updates/releases/"),
        "更新包必须来自当前服务端的 /updates/releases/ 目录"
    );
    Ok(url)
}
fn release_version(value: &str) -> Result<Version> {
    let version = Version::parse(value)?;
    ensure!(version.pre.is_empty(), "只允许正式版本");
    Ok(version)
}
fn verify_signature(bytes: &[u8], signature: &str) -> Result<()> {
    // Reuse the existing release signing identity. Private keys never ship here.
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../desktop/src-tauri/tauri.conf.json"))?;
    let key = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .context("缺少发布公钥")?;
    verify_with_key(bytes, signature, key)
}
fn verify_with_key(bytes: &[u8], signature: &str, key: &str) -> Result<()> {
    let public = String::from_utf8(STANDARD.decode(key.trim())?)?;
    let signature = String::from_utf8(STANDARD.decode(signature.trim())?)?;
    minisign_verify::PublicKey::decode(&public)?
        .verify(
            bytes,
            &minisign_verify::Signature::decode(&signature)?,
            true,
        )
        .context("更新包签名校验失败")
}
async fn download(client: &reqwest::Client, url: Url, limit: usize) -> Result<Vec<u8>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "服务端未返回更新文件"
    );
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "更新文件过大"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "更新文件过大"
        );
        bytes.extend_from_slice(&chunk);
    }
    ensure!(!bytes.is_empty(), "更新文件为空");
    Ok(bytes)
}
async fn binary_version(path: &Path) -> Result<Version> {
    let mut command = tokio::process::Command::new(path);
    command.arg("--version").kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
    ensure!(output.status.success(), "客户端无法执行 --version");
    let text = std::str::from_utf8(&output.stdout)?.trim();
    release_version(
        text.strip_prefix("vpn-cli ")
            .context("不是 vpn-cli 可执行文件")?,
    )
}
async fn systemctl(args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new("systemctl");
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output()).await??;
    ensure!(
        output.status.success(),
        "systemctl {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
async fn request(req: IpcRequest) -> Result<IpcResponse> {
    Ok(tokio::time::timeout(
        Duration::from_secs(3),
        ipc::send_request(Path::new("/run/vpn-cli/vpn-cli.sock"), &req),
    )
    .await??)
}

/// systemd ExecStop waits for the daemon to finish route and DNS cleanup.
pub async fn drain() -> Result<()> {
    // Wait for route/DNS cleanup before systemd sends SIGTERM. If a
    // running daemon is unresponsive, fail closed instead of leaving
    // network state behind while replacing its executable.
    ensure!(
        matches!(request(IpcRequest::Disconnect).await?, IpcResponse::Ok),
        "无法请求断开 VPN"
    );
    let mut disconnected = false;
    for _ in 0..50 {
        if matches!(request(IpcRequest::GetStatus).await?, IpcResponse::Status(s) if s.state == ConnState::Disconnected)
        {
            disconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    ensure!(disconnected, "VPN 路由清理未完成，已中止升级");
    Ok(())
}

#[async_trait::async_trait]
trait Service: Sync {
    async fn active(&self) -> Result<bool>;
    async fn stop(&self) -> Result<()>;
    async fn start(&self) -> Result<()>;
    async fn healthy(&self) -> Result<()>;
    async fn stop_for_recovery(&self) -> Result<()> {
        self.stop().await
    }
}
struct SystemService;
#[async_trait::async_trait]
impl Service for SystemService {
    async fn stop_for_recovery(&self) -> Result<()> {
        // A broken replacement may never expose IPC. Try graceful cleanup,
        // then stop it through systemd so it cannot prevent rollback forever.
        if self.stop().await.is_err() {
            if let Err(error) = systemctl(&["stop", "vpn-cli.service"]).await {
                // ExecStop uses the replacement binary too and may fail.
                // A failed stop job is tolerable only after systemd confirms
                // that no process remains; otherwise preserve the journal.
                let pid = systemctl(&["show", "--property=MainPID", "--value", "vpn-cli.service"])
                    .await?;
                let state = systemctl(&[
                    "show",
                    "--property=ActiveState",
                    "--value",
                    "vpn-cli.service",
                ])
                .await?;
                ensure!(
                    pid == "0" && matches!(state.as_str(), "inactive" | "failed"),
                    "回滚前无法停止新客户端：{error:#}"
                );
            }
        }
        Ok(())
    }
    async fn active(&self) -> Result<bool> {
        let state = systemctl(&[
            "show",
            "--property=ActiveState",
            "--value",
            "vpn-cli.service",
        ])
        .await?;
        Ok(matches!(
            state.as_str(),
            "active" | "activating" | "reloading"
        ))
    }
    async fn stop(&self) -> Result<()> {
        if self.active().await? {
            drain().await?;
        }
        systemctl(&["stop", "vpn-cli.service"]).await?;
        Ok(())
    }
    async fn start(&self) -> Result<()> {
        systemctl(&["start", "vpn-cli.service"]).await?;
        Ok(())
    }
    async fn healthy(&self) -> Result<()> {
        // Process + IPC health, independent of VPN/network availability. An
        // authorization/version error should not trigger a downgrade loop.
        let mut stable = 0;
        let mut previous_pid = String::new();
        for _ in 0..30 {
            let pid =
                systemctl(&["show", "--property=MainPID", "--value", "vpn-cli.service"]).await?;
            if pid != "0"
                && pid == previous_pid
                && self.active().await?
                && matches!(
                    request(IpcRequest::GetStatus).await,
                    Ok(IpcResponse::Status(_))
                )
            {
                stable += 1;
                if stable >= 3 {
                    return Ok(());
                }
            } else {
                stable = 0;
            }
            previous_pid = pid;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("新客户端未能稳定运行或 IPC 不可用")
    }
}

async fn recover(config: &Config, root: &Path, service: &impl Service) -> Result<bool> {
    let journal = root.join("pending.json");
    if !journal.exists() {
        return Ok(false);
    }
    let pending: Pending = serde_json::from_slice(&fs::read(&journal)?)?;
    // Keep the journal/backup until rollback AND restart have succeeded; a
    // subsequent timer run retries recovery even if the installed binary broke.
    service.stop_for_recovery().await?;
    let backup = fs::read(root.join("previous.bin")).context("升级回滚备份缺失")?;
    atomic_write(&config.binary, &backup, 0o755)?;
    if pending.was_active {
        service.start().await?;
        service.healthy().await?;
    }
    atomic_write(
        &root.join("rejected-version"),
        pending.version.as_bytes(),
        0o600,
    )?;
    remove_synced(&journal)?;
    eprintln!(
        "已回滚升级 {}；该版本不再自动重试，等待下一正式版本",
        pending.version
    );
    Ok(true)
}
async fn apply(
    config: &Config,
    root: &Path,
    staged: &Path,
    version: &str,
    service: &impl Service,
) -> Result<()> {
    let pending = Pending {
        version: version.into(),
        was_active: service.active().await?,
    };
    atomic_write(
        &root.join("previous.bin"),
        &fs::read(&config.binary)?,
        0o700,
    )?;
    atomic_write(
        &root.join("pending.json"),
        &serde_json::to_vec(&pending)?,
        0o600,
    )?;
    let result: Result<()> = async {
        service.stop().await?;
        fs::rename(staged, &config.binary)?;
        File::open(config.binary.parent().context("缺少父目录")?)?.sync_all()?;
        if pending.was_active {
            service.start().await?;
            service.healthy().await?;
        }
        remove_synced(&root.join("pending.json"))?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        recover(config, root, service)
            .await
            .context(format!("升级失败：{error:#}；回滚也未完成"))?;
        return Err(error.context("升级失败，已回滚"));
    }
    println!("已升级 Linux CLI 至 {version}");
    Ok(())
}

fn daemon_unit(binary: &Path) -> Result<String> {
    let path = binary.to_str().context("可执行文件路径必须为 UTF-8")?;
    ensure!(
        path.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b)),
        "系统服务可执行文件路径仅支持字母、数字、斜线、连字符、下划线和点"
    );
    Ok(format!("[Unit]\nDescription=VPN CLI unattended node\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nEnvironment=VPN_CLI_CRED_BACKEND=file\nEnvironment=VPN_CLI_AUTO_CONNECT=1\nEnvironment=HOME=/root\nEnvironment=XDG_CONFIG_HOME=/root/.config\nEnvironment=XDG_RUNTIME_DIR=/run/vpn-cli\nRuntimeDirectory=vpn-cli\nRuntimeDirectoryMode=0700\nExecStart={path} daemon run\nExecStop={path} daemon drain\nRestart=on-failure\nRestartSec=5\nTimeoutStopSec=60\n\n[Install]\nWantedBy=multi-user.target\n"))
}

pub fn install_daemon() -> Result<()> {
    require_root()?;
    let binary = std::env::current_exe()?;
    trusted_path(&binary)?;
    let unit = daemon_unit(&binary)?;
    if Path::new(UNIT).exists() {
        ensure!(
            fs::read_to_string(UNIT)? == unit,
            "已有不同的 vpn-cli 系统服务，请先按文档迁移，避免覆盖配置"
        );
    }
    atomic_write(Path::new(UNIT), unit.as_bytes(), 0o644)?;
    let output = std::process::Command::new("systemctl")
        .arg("daemon-reload")
        .output()?;
    ensure!(output.status.success(), "systemctl daemon-reload 失败");
    let output = std::process::Command::new("systemctl")
        .args(["enable", "vpn-cli.service"])
        .output()?;
    ensure!(output.status.success(), "systemctl enable 失败");
    Ok(())
}

async fn install(server: &str, schedule: &str) -> Result<()> {
    endpoint(server)?;
    ensure!(
        !schedule.is_empty() && !schedule.chars().any(char::is_control),
        "无效更新时间表达式"
    );
    // Let systemd validate before changing any files; shell is never involved.
    let output = tokio::process::Command::new("systemd-analyze")
        .args(["calendar", "--", schedule])
        .output()
        .await?;
    ensure!(output.status.success(), "无效 systemd OnCalendar 表达式");
    let binary = std::env::current_exe()?;
    trusted_path(&binary)?;
    ensure!(
        binary != Path::new(RUNNER),
        "请通过主 vpn-cli 程序安装更新任务"
    );
    ensure!(
        fs::read_to_string(UNIT).context("请先执行 vpn-cli daemon install-system")?
            == daemon_unit(&binary)?,
        "自动升级只支持由 daemon install-system 创建且使用当前可执行文件的系统服务"
    );
    ensure!(
        !Path::new(ROOT).join("pending.json").exists(),
        "存在未完成升级，请先执行 update run 恢复"
    );
    secure_dir(Path::new("/usr/local/libexec"))?;
    atomic_write(Path::new(RUNNER), &fs::read(&binary)?, 0o755)?;
    let config = Config {
        server: server.into(),
        binary,
    };
    atomic_write(
        Path::new(CONFIG),
        &serde_json::to_vec_pretty(&config)?,
        0o600,
    )?;
    atomic_write(Path::new(UPDATE_UNIT), format!("[Unit]\nDescription=VPN CLI signed updater\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=oneshot\nEnvironment=HOME=/root\nEnvironment=XDG_CONFIG_HOME=/root/.config\nEnvironment=XDG_RUNTIME_DIR=/run/vpn-cli\nExecStart={RUNNER} update run\nTimeoutStartSec=15min\nUMask=0077\n").as_bytes(), 0o644)?;
    atomic_write(Path::new(TIMER), format!("[Unit]\nDescription=VPN CLI update schedule\n\n[Timer]\nOnCalendar={schedule}\nPersistent=true\nRandomizedDelaySec=5min\n\n[Install]\nWantedBy=timers.target\n").as_bytes(), 0o644)?;
    systemctl(&["daemon-reload"]).await?;
    systemctl(&["enable", "--now", "vpn-cli-update.timer"]).await?;
    systemctl(&["restart", "vpn-cli-update.timer"]).await?;
    println!(
        "自动升级已启用（{schedule}，随机延迟最多 5 分钟）。日志：journalctl -u vpn-cli-update"
    );
    Ok(())
}

async fn run() -> Result<()> {
    trusted_path(Path::new(CONFIG))?;
    let config: Config = serde_json::from_slice(&fs::read(CONFIG)?)?;
    trusted_path(&config.binary)?;
    ensure!(
        fs::read_to_string(UNIT)? == daemon_unit(&config.binary)?,
        "系统服务配置已改变，请检查配置后重新安装更新任务"
    );
    let root = Path::new(ROOT);
    if recover(&config, root, &SystemService).await? {
        return Ok(());
    }
    let manifest_url = endpoint(&config.server)?;
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()?;
    let manifest: Manifest =
        serde_json::from_slice(&download(&client, manifest_url.clone(), 1024 * 1024).await?)?;
    let target_version = release_version(&manifest.version)?;
    let current = binary_version(&config.binary).await?;
    if !target_version.cmp_precedence(&current).is_gt() {
        println!("当前 CLI 已是最新版本 ({current})");
        return Ok(());
    }
    if fs::read_to_string(root.join("rejected-version"))
        .ok()
        .as_deref()
        == Some(&manifest.version)
    {
        bail!("版本 {} 曾升级失败，已暂停该版本自动重试", manifest.version);
    }
    let target = format!("cli-linux-{}", std::env::consts::ARCH);
    let package = manifest
        .platforms
        .get(&target)
        .context("服务端尚未提供当前架构的 CLI 签名更新包")?;
    let bytes = download(
        &client,
        package_url(&manifest_url, &package.url)?,
        MAX_BINARY,
    )
    .await?;
    verify_signature(&bytes, &package.signature)?;
    let staged = config.binary.with_extension("update-new");
    atomic_write(&staged, &bytes, 0o755)?;
    let result = async {
        ensure!(
            binary_version(&staged).await? == target_version,
            "签名二进制版本与清单不匹配"
        );
        apply(&config, root, &staged, &manifest.version, &SystemService).await
    }
    .await;
    if staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

pub async fn dispatch(action: &crate::cli::UpdateAction) -> Result<()> {
    require_root()?;
    secure_dir(Path::new(ROOT))?;
    let _lock = lock(Path::new(ROOT))?;
    match action {
        crate::cli::UpdateAction::Install { server, schedule } => install(server, schedule).await,
        crate::cli::UpdateAction::Run => run().await,
        crate::cli::UpdateAction::Disable => {
            systemctl(&["disable", "--now", "vpn-cli-update.timer"]).await?;
            println!("自动升级定时任务已关闭");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeService {
        running: AtomicBool,
        starts: AtomicUsize,
        bad_health: AtomicBool,
        refuse_start: AtomicBool,
    }
    impl FakeService {
        fn new(active: bool, fail_once: bool) -> Self {
            Self {
                running: AtomicBool::new(active),
                starts: AtomicUsize::new(0),
                bad_health: AtomicBool::new(fail_once),
                refuse_start: AtomicBool::new(false),
            }
        }
    }
    #[async_trait::async_trait]
    impl Service for FakeService {
        async fn active(&self) -> Result<bool> {
            Ok(self.running.load(Ordering::SeqCst))
        }
        async fn stop(&self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);
            Ok(())
        }
        async fn start(&self) -> Result<()> {
            ensure!(
                !self.refuse_start.load(Ordering::SeqCst),
                "systemd unavailable"
            );
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn healthy(&self) -> Result<()> {
            ensure!(
                !self.bad_health.swap(false, Ordering::SeqCst),
                "broken replacement"
            );
            Ok(())
        }
    }
    fn fixture() -> (tempfile::TempDir, Config, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("vpn-cli");
        let staged = dir.path().join("vpn-cli.update-new");
        fs::write(&binary, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        let config = Config {
            server: "https://vpn.example".into(),
            binary,
        };
        (dir, config, staged)
    }
    #[tokio::test]
    async fn healthy_update_restarts_and_keeps_backup() {
        let (dir, config, staged) = fixture();
        let service = FakeService::new(true, false);
        apply(&config, dir.path(), &staged, "1.2.0", &service)
            .await
            .unwrap();
        assert_eq!(fs::read(&config.binary).unwrap(), b"new");
        assert_eq!(fs::read(dir.path().join("previous.bin")).unwrap(), b"old");
        assert!(!dir.path().join("pending.json").exists());
        assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn unhealthy_update_restores_and_quarantines_version() {
        let (dir, config, staged) = fixture();
        let service = FakeService::new(true, true);
        assert!(apply(&config, dir.path(), &staged, "1.2.0", &service)
            .await
            .is_err());
        assert_eq!(fs::read(&config.binary).unwrap(), b"old");
        assert_eq!(
            fs::read_to_string(dir.path().join("rejected-version")).unwrap(),
            "1.2.0"
        );
        assert_eq!(service.starts.load(Ordering::SeqCst), 2);
        assert!(!dir.path().join("pending.json").exists());
    }
    #[tokio::test]
    async fn failed_rollback_keeps_journal_for_independent_runner() {
        let (dir, config, staged) = fixture();
        let service = FakeService::new(true, false);
        service.refuse_start.store(true, Ordering::SeqCst);
        assert!(apply(&config, dir.path(), &staged, "1.2.0", &service)
            .await
            .is_err());
        assert!(dir.path().join("pending.json").exists());
        service.refuse_start.store(false, Ordering::SeqCst);
        assert!(recover(&config, dir.path(), &service).await.unwrap());
        assert_eq!(fs::read(&config.binary).unwrap(), b"old");
        assert!(service.active().await.unwrap());
        assert!(!dir.path().join("pending.json").exists());
    }
    #[tokio::test]
    async fn stopped_service_is_not_started_by_update() {
        let (dir, config, staged) = fixture();
        let service = FakeService::new(false, false);
        apply(&config, dir.path(), &staged, "1.2.0", &service)
            .await
            .unwrap();
        assert_eq!(service.starts.load(Ordering::SeqCst), 0);
        assert!(!service.active().await.unwrap());
    }
    #[tokio::test]
    async fn interruption_after_rename_recovers_without_executing_broken_binary() {
        let (dir, config, staged) = fixture();
        atomic_write(&dir.path().join("previous.bin"), b"old", 0o700).unwrap();
        atomic_write(
            &dir.path().join("pending.json"),
            br#"{"version":"1.2.0","was_active":true}"#,
            0o600,
        )
        .unwrap();
        fs::rename(staged, &config.binary).unwrap();
        let service = FakeService::new(false, false);
        recover(&config, dir.path(), &service).await.unwrap();
        assert_eq!(fs::read(&config.binary).unwrap(), b"old");
        assert!(service.active().await.unwrap());
    }
    #[test]
    fn locks_exclude_concurrent_runs_and_release_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = lock(dir.path()).unwrap();
        assert!(lock(dir.path()).is_err());
        drop(first);
        assert!(lock(dir.path()).is_ok());
    }
    #[test]
    fn updates_are_https_same_origin_and_stable_versions() {
        let source = endpoint("https://vpn.example:8443").unwrap();
        assert_eq!(
            source.as_str(),
            "https://vpn.example:8443/updates/latest.json"
        );
        for bad in [
            "http://vpn.example",
            "https://user@vpn.example",
            "https://vpn.example/path",
            "https://vpn.example?q=1",
        ] {
            assert!(endpoint(bad).is_err(), "{bad}");
        }
        for bad in [
            "https://evil.example/updates/releases/a.bin",
            "http://vpn.example:8443/updates/releases/a.bin",
            "https://vpn.example:8443/other/a.bin",
        ] {
            assert!(package_url(&source, bad).is_err(), "{bad}");
        }
        assert!(package_url(&source, "https://vpn.example:8443/updates/releases/a/x.bin").is_ok());
        assert!(release_version("1.2.0-beta.1").is_err());
        assert!(release_version("1.2.0")
            .unwrap()
            .cmp_precedence(&release_version("1.2.0+metadata").unwrap())
            .is_eq());
    }
    #[test]
    fn verifies_real_signature_and_rejects_tampering_and_wrong_key() {
        // Public Minisign test vector (no private signing material).
        let key = STANDARD.encode(
            "untrusted comment: test key\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
        );
        let sig = STANDARD.encode("untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==");
        verify_with_key(b"test", &sig, &key).unwrap();
        assert!(verify_with_key(b"Test", &sig, &key).is_err());
        assert!(verify_signature(b"test", &sig).is_err());
        assert!(verify_signature(b"test", "broken base64").is_err());
    }
    #[tokio::test]
    async fn downloads_reject_redirect_empty_and_oversized_responses() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for (path, response) in [
            (
                "/redirect",
                ResponseTemplate::new(302).insert_header("Location", "/ok"),
            ),
            ("/empty", ResponseTemplate::new(200)),
            (
                "/large",
                ResponseTemplate::new(200).set_body_bytes(vec![42; 20]),
            ),
            (
                "/ok",
                ResponseTemplate::new(200).set_body_bytes(vec![42; 5]),
            ),
        ] {
            Mock::given(wiremock::matchers::path(path))
                .respond_with(response)
                .mount(&server)
                .await;
            let result = download(
                &client,
                Url::parse(&format!("{}{path}", server.uri())).unwrap(),
                10,
            )
            .await;
            assert_eq!(result.is_ok(), path == "/ok", "{path}");
        }
    }
    #[test]
    fn daemon_unit_rejects_systemd_injection_and_sets_headless_context() {
        for bad in ["/usr/bin/vpn cli", "/usr/bin/vpn%cli", "/usr/bin/vpn\ncli"] {
            assert!(daemon_unit(Path::new(bad)).is_err());
        }
        let unit = daemon_unit(Path::new("/usr/local/bin/vpn-cli")).unwrap();
        assert!(unit.contains("VPN_CLI_CRED_BACKEND=file"));
        assert!(unit.contains("VPN_CLI_AUTO_CONNECT=1"));
        assert!(unit.contains("RuntimeDirectoryMode=0700"));
    }
}
