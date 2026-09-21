//! 在产品自有 TUN 接口上应用、恢复服务端下发的 DNS 策略。

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use hickory_proto::{
    op::{Message, MessageType, OpCode, Query, ResponseCode},
    rr::{Name, RecordType},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    process::Command,
    sync::watch,
    time::timeout,
};
use vpn_api_types::{peer::ClientDnsSettings, system::ClientDnsMode};

use crate::{PlatformError, Result};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
// Each Windows operation gets its own budget, including PowerShell/CIM cold start.
const WINDOWS_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
// 允许服务端最多 8 个上游依次故障切换（每个总预算 2 秒），另留 2 秒隧道开销。
const PROBE_TIMEOUT: Duration = Duration::from_secs(18);

const OWNER: &str = "com.xeflow.yilian.vpn";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum DnsPlatform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandSpec {
    stage: &'static str,
    program: &'static str,
    args: Vec<String>,
    stdin: Option<String>,
}

/// 一次 DNS 应用会话；只恢复本产品在当前 TUN 接口上创建的状态。
pub struct DnsSession {
    platform: DnsPlatform,
    ifindex: u32,
    interface_name: String,
    active: bool,
    #[cfg(target_os = "windows")]
    lease: Option<tokio::process::Child>,
}

impl DnsSession {
    pub async fn restore(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        if let Some(mut lease) = self.lease.take() {
            drop(lease.stdin.take());
            // Reap the lease before a replacement can reuse the interface.
            match timeout(WINDOWS_COMMAND_TIMEOUT, lease.wait()).await {
                Ok(Ok(status)) if status.success() => {}
                _ => {
                    let _ = lease.kill().await;
                }
            }
        }
        run_commands(cleanup_commands(
            self.platform,
            self.ifindex,
            &self.interface_name,
        ))
        .await?;
        self.active = false;
        tracing::info!(
            stage = "dns_restore",
            result = "succeeded",
            "客户端 DNS 已恢复"
        );
        Ok(())
    }
}

/// 应用 DNS。开始前先清理上次异常退出可能遗留的产品状态；失败时立即回滚。
pub async fn apply_dns(ifindex: u32, settings: &ClientDnsSettings) -> Result<Option<DnsSession>> {
    if settings.mode == ClientDnsMode::Disabled {
        cleanup_stale_dns(ifindex).await?;
        return Ok(None);
    }
    let server: Ipv4Addr = settings.server.parse().map_err(|_| {
        PlatformError::InvalidArgument(format!("服务端下发了非法 DNS 地址：{}", settings.server))
    })?;
    let platform = current_platform()?;
    let interface_name = interface_name(ifindex)?;
    run_commands(cleanup_commands(platform, ifindex, &interface_name)).await?;
    #[cfg(target_os = "windows")]
    let (applied, lease) = match start_windows_lease(ifindex, server).await {
        Ok(child) => (Ok(()), Some(child)),
        Err(error) => (Err(error), None),
    };
    #[cfg(not(target_os = "windows"))]
    let applied = run_commands(apply_commands(platform, ifindex, &interface_name, server)).await;
    if let Err(error) = applied {
        let rollback = run_commands(cleanup_commands(platform, ifindex, &interface_name)).await;
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(PlatformError::Command {
                command: "dns rollback".to_string(),
                message: format!("应用失败：{error}；回滚失败：{rollback}"),
            }),
        };
    }
    tracing::info!(stage = "dns_apply", result = "succeeded", ?settings.mode, "客户端全局 DNS 已应用");
    Ok(Some(DnsSession {
        platform,
        ifindex,
        interface_name,
        active: true,
        #[cfg(target_os = "windows")]
        lease,
    }))
}

fn windows_lease_script(ifindex: u32, server: Ipv4Addr) -> String {
    include_str!("dns/windows_lease.ps1")
        .replace("__IFINDEX__", &ifindex.to_string())
        .replace("__SERVER__", &server.to_string())
}

#[cfg(target_os = "windows")]
async fn start_windows_lease(ifindex: u32, server: Ipv4Addr) -> Result<tokio::process::Child> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let spec = apply_commands(DnsPlatform::Windows, ifindex, "", server).remove(0);
    let mut child = Command::new(spec.program)
        .args(spec.args)
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Once ready, EOF (including client death) must execute PowerShell finally.
        .kill_on_drop(false)
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    match timeout(WINDOWS_COMMAND_TIMEOUT, reader.read_line(&mut line)).await {
        Ok(Ok(_)) if line.trim() == "ready" => Ok(child),
        _ => {
            drop(child.stdin.take());
            // Do not return while an old worker could still reset a new interface.
            let _ = child.kill().await;
            let mut detail = String::new();
            if let Some(stderr) = child.stderr.take() {
                let _ = timeout(
                    COMMAND_TIMEOUT,
                    stderr.take(4096).read_to_string(&mut detail),
                )
                .await;
            }
            Err(PlatformError::command(
                "dns lease",
                format!("Windows DNS 租约启动失败或超时：{}", detail.trim()),
            ))
        }
    }
}

/// 清理上次进程异常退出可能遗留的持久产品状态。Linux link DNS 随 TUN 消失，无需处理。
pub async fn cleanup_stale_dns(ifindex: u32) -> Result<()> {
    let platform = current_platform()?;
    let interface_name = interface_name(ifindex)?;
    run_commands(stale_cleanup_commands(platform, ifindex, &interface_name)).await
}

/// 在控制面请求/域名解析之前调用；调用者必须保证没有本进程的活动隧道。
/// 不依赖 TUN 是否存在，也不更改物理网卡 DNS。
pub async fn cleanup_dns_before_connect() -> Result<()> {
    match current_platform()? {
        DnsPlatform::Windows => run_commands(vec![windows_policy_cleanup_command()]).await,
        // macOS 的动态配置由特权 helper 管理；Linux 配置随 link 消失。
        DnsPlatform::Macos => run_commands(cleanup_commands(DnsPlatform::Macos, 0, "")).await,
        DnsPlatform::Linux => Ok(()),
    }
}

fn windows_policy_cleanup_command() -> CommandSpec {
    let mut command = powershell(windows_policy_cleanup());
    command.stage = "清理 NRPT 策略/刷新缓存";
    command
}

fn windows_interface_cleanup(ifindex: u32) -> CommandSpec {
    let mut command = powershell(
        include_str!("dns/windows_interface_cleanup.ps1")
            .replace("__IFINDEX__", &ifindex.to_string()),
    );
    command.stage = "重置 VPN 网卡 DNS";
    command
}

fn windows_policy_cleanup() -> String {
    include_str!("dns/windows_cleanup.ps1").to_string()
}

/// 转发循环启动后并行运行。只有实际解析成功才接管系统 DNS；故障时恢复原解析器。
/// 此任务必须通过 shutdown 退出并等待完成，不能 abort，否则无法等待系统清理。
pub async fn monitor_dns(
    ifindex: u32,
    local_ip: Ipv4Addr,
    settings: ClientDnsSettings,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if settings.mode == ClientDnsMode::Disabled {
        return Ok(());
    }
    let server: Ipv4Addr = settings
        .server
        .parse()
        .map_err(|_| PlatformError::InvalidArgument("非法 DNS 地址".into()))?;
    let mut backend = SystemDns {
        ifindex,
        local_ip,
        server,
        settings,
        session: None,
    };
    maintain_dns(&mut backend, shutdown).await
}

#[async_trait::async_trait]
trait DnsMaintenance: Send {
    async fn probe(&mut self) -> Result<()>;
    async fn apply(&mut self) -> Result<()>;
    async fn restore(&mut self) -> Result<()>;
}

struct SystemDns {
    ifindex: u32,
    local_ip: Ipv4Addr,
    server: Ipv4Addr,
    settings: ClientDnsSettings,
    session: Option<DnsSession>,
}

#[async_trait::async_trait]
impl DnsMaintenance for SystemDns {
    async fn probe(&mut self) -> Result<()> {
        #[cfg(target_os = "windows")]
        if let Some(session) = self.session.as_mut() {
            if let Some(lease) = session.lease.as_mut() {
                if lease.try_wait()?.is_some() {
                    return Err(PlatformError::command("dns lease", "DNS 租约进程意外退出"));
                }
            }
        }
        probe_dns(self.local_ip, SocketAddr::from((self.server, 53))).await
    }
    async fn apply(&mut self) -> Result<()> {
        self.session = apply_dns(self.ifindex, &self.settings).await?;
        Ok(())
    }
    async fn restore(&mut self) -> Result<()> {
        if let Some(active) = self.session.as_mut() {
            active.restore().await?;
        }
        self.session = None;
        Ok(())
    }
}

async fn maintain_dns(
    backend: &mut impl DnsMaintenance,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut active = false;
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let outcome = async {
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                _ = ticker.tick() => {}
            }
            let healthy = tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                result = backend.probe() => result.is_ok(),
            };
            if healthy && !active {
                backend.apply().await?;
                active = true;
            } else if !healthy && active {
                backend.restore().await?;
                active = false;
                tracing::warn!(
                    stage = "dns_health",
                    "VPN DNS 解析失败，已恢复系统原有 DNS，等待恢复"
                );
            } else if !healthy {
                tracing::debug!(stage = "dns_health", "VPN DNS 尚未就绪，保留系统原有 DNS");
            }
        }
        Ok(())
    }
    .await;
    // 错误退出也执行恢复；失败的 restore 会在这里再尝试一次。
    backend.restore().await?;
    outcome
}

async fn probe_dns(local_ip: Ipv4Addr, server: SocketAddr) -> Result<()> {
    timeout(PROBE_TIMEOUT, async {
        let mut query = Message::query();
        query.metadata.id = rand::random();
        query.metadata.recursion_desired = true;
        // 根 NS 查询检验默认上游，避免只测到网关静态记录或泄露用户域名。
        query.add_query(Query::query(Name::root(), RecordType::NS));
        let packet = query
            .to_vec()
            .map_err(|e| PlatformError::command("dns probe", e.to_string()))?;
        let socket = UdpSocket::bind((local_ip, 0)).await?;
        socket.connect(server).await?;
        socket.send(&packet).await?;
        let mut buffer = [0u8; 4096];
        let size = socket.recv(&mut buffer).await?;
        let reply = Message::from_vec(&buffer[..size])
            .map_err(|e| PlatformError::command("dns probe", e.to_string()))?;
        if reply.id != query.id
            || reply.message_type != MessageType::Response
            || reply.op_code != OpCode::Query
            || reply.queries != query.queries
            || reply.response_code != ResponseCode::NoError
            || reply.truncation
            || reply.answers.is_empty()
        {
            return Err(PlatformError::command(
                "dns probe",
                "DNS 未返回有效解析结果",
            ));
        }
        Ok(())
    })
    .await
    .map_err(|_| PlatformError::command("dns probe", "DNS 查询超时"))?
}

fn stale_cleanup_commands(
    platform: DnsPlatform,
    ifindex: u32,
    interface: &str,
) -> Vec<CommandSpec> {
    match platform {
        DnsPlatform::Linux => Vec::new(),
        _ => cleanup_commands(platform, ifindex, interface),
    }
}

async fn run_commands(commands: Vec<CommandSpec>) -> Result<()> {
    for spec in commands {
        let mut command = Command::new(spec.program);
        command.args(&spec.args);
        command.kill_on_drop(true);
        // DNS maintenance runs in the background, including cleanup on every connection.
        // Redirecting stdio alone does not prevent PowerShell from opening a console.
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        if spec.stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        let duration = if spec.program == "powershell.exe" {
            WINDOWS_COMMAND_TIMEOUT
        } else {
            COMMAND_TIMEOUT
        };
        let stderr = child.stderr.take();
        let output = timeout(duration, async {
            if let Some(input) = spec.stdin {
                child
                    .stdin
                    .take()
                    .ok_or_else(|| PlatformError::command(spec.program, "无法打开 stdin"))?
                    .write_all(input.as_bytes())
                    .await?;
            }
            let (status, stderr) = tokio::join!(child.wait(), drain_stderr(stderr));
            Ok::<_, PlatformError>(std::process::Output {
                status: status?,
                stdout: Vec::new(),
                stderr: stderr?,
            })
        })
        .await;
        let output = match output {
            Ok(result) => result?,
            Err(_) => {
                // Explicitly kill AND reap before another attempt can reuse the interface.
                child.kill().await.map_err(|error| {
                    PlatformError::command(
                        spec.program,
                        format!("{}超时，终止子进程失败：{error}", spec.stage),
                    )
                })?;
                return Err(PlatformError::command(spec.program,format!("{}超时（{} 秒），已终止并回收子进程；请检查 Windows DNS Client/WMI 服务后重试",spec.stage,duration.as_secs())));
            }
        };
        if !output.status.success() {
            return Err(PlatformError::command(
                spec.program,
                format!(
                    "{}失败：{}",
                    spec.stage,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
    }
    Ok(())
}

/// Drain the pipe even beyond the diagnostic limit, so a noisy child cannot deadlock.
async fn drain_stderr(stderr: Option<tokio::process::ChildStderr>) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    if let Some(mut stderr) = stderr {
        let mut buffer = [0u8; 4096];
        loop {
            let count = stderr.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..count.min(4096 - output.len())]);
        }
    }
    Ok(output)
}

fn cleanup_commands(platform: DnsPlatform, ifindex: u32, interface: &str) -> Vec<CommandSpec> {
    match platform {
        DnsPlatform::Linux => vec![spec("resolvectl", ["revert", interface])],
        DnsPlatform::Macos => vec![CommandSpec {
            stage: "macOS DNS 配置",
            program: "/usr/sbin/scutil",
            args: vec![],
            stdin: Some(format!("remove State:/Network/Service/{OWNER}/DNS\nquit\n")),
        }],
        DnsPlatform::Windows => vec![
            windows_policy_cleanup_command(),
            windows_interface_cleanup(ifindex),
        ],
    }
}

fn apply_commands(
    platform: DnsPlatform,
    ifindex: u32,
    interface: &str,
    server: Ipv4Addr,
) -> Vec<CommandSpec> {
    match platform {
        DnsPlatform::Linux => vec![
            spec("resolvectl", ["dns", interface, &server.to_string()]),
            spec("resolvectl", ["domain", interface, "~."]),
        ],
        // 空 match domain 将产品自有 VPN DNS 注册为默认解析器。
        DnsPlatform::Macos => vec![CommandSpec {
                stage: "macOS DNS 配置",
            program: "/usr/sbin/scutil",
                args: vec![],
                stdin: Some(format!(
                    "d.init\nd.add ServerAddresses * {server}\nd.add InterfaceName {interface}\nd.add SearchOrder # 1\nd.add SupplementalMatchDomains * \"\"\nd.add SupplementalMatchDomainsNoSearch # 1\nset State:/Network/Service/{OWNER}/DNS\nquit\n"
                )),
            }],
        DnsPlatform::Windows => vec![powershell(windows_lease_script(ifindex, server))],
    }
}

fn spec<const N: usize>(program: &'static str, args: [&str; N]) -> CommandSpec {
    CommandSpec {
        stage: "DNS 配置",
        program,
        args: args.into_iter().map(str::to_string).collect(),
        stdin: None,
    }
}

fn powershell(script: String) -> CommandSpec {
    CommandSpec {
        stage: "Windows DNS 配置",
        program: "powershell.exe",
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            script,
        ],
        stdin: None,
    }
}

fn current_platform() -> Result<DnsPlatform> {
    #[cfg(target_os = "linux")]
    return Ok(DnsPlatform::Linux);
    #[cfg(target_os = "macos")]
    return Ok(DnsPlatform::Macos);
    #[cfg(target_os = "windows")]
    return Ok(DnsPlatform::Windows);
    #[allow(unreachable_code)]
    Err(PlatformError::Unsupported(
        "DNS 配置仅支持 Linux、macOS 和 Windows".to_string(),
    ))
}

#[cfg(unix)]
fn interface_name(ifindex: u32) -> Result<String> {
    let mut buffer = [0 as libc::c_char; libc::IF_NAMESIZE];
    let pointer = unsafe { libc::if_indextoname(ifindex, buffer.as_mut_ptr()) };
    if pointer.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let name = unsafe { std::ffi::CStr::from_ptr(pointer) };
    Ok(name.to_string_lossy().into_owned())
}

#[cfg(target_os = "windows")]
fn interface_name(ifindex: u32) -> Result<String> {
    Ok(ifindex.to_string())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn interface_name(_ifindex: u32) -> Result<String> {
    Err(PlatformError::Unsupported("无法读取接口名".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn windows_dns_commands_run_without_a_console() {
        // Exercise the actual process runner without changing system DNS or requiring elevation.
        run_commands(vec![powershell(
            "$ErrorActionPreference='Stop'; Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class ConsoleProbe { [DllImport(\"kernel32.dll\")] public static extern IntPtr GetConsoleWindow(); }'; if ([ConsoleProbe]::GetConsoleWindow() -ne [IntPtr]::Zero) { throw 'DNS command has a console window' }".into(),
        )])
        .await
        .unwrap();
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn empty_nrpt_store_skips_dns_cim_commands() {
        // Exercise the real PowerShell control flow with an isolated HKCU key.
        // No elevation, real NRPT policy, or network adapter changes are required.
        let key = format!("Software\\YilianDnsCleanupTest{}", std::process::id());
        let cleanup = windows_policy_cleanup()
            .replace("Registry]::LocalMachine", "Registry]::CurrentUser")
            .replace(
                r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig",
                &key,
            );
        let script = format!(
            r#"
$ErrorActionPreference='Stop'
function Get-DnsClientNrptRule {{ throw 'Unnecessary NRPT enumeration' }}
function Clear-DnsClientCache {{ throw 'Unnecessary cache flush' }}
try {{
    & {{ {cleanup} }}
    $testKey=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('{key}')
    $testKey.Dispose()
    & {{ {cleanup} }}
}} finally {{ [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey('{key}', $false) }}
"#
        );
        run_commands(vec![powershell(script)]).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_command_diagnostics_are_bounded_and_identify_stage() {
        let error = run_commands(vec![spec(
            "/bin/sh",
            [
                "-c",
                "i=0; while [ $i -lt 5000 ]; do echo diagnostic >&2; i=$((i+1)); done; exit 1",
            ],
        )])
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("DNS 配置失败"));
        assert!(error.contains("diagnostic"));
        assert!(error.len() < 4500);
    }

    #[test]
    fn disabled_or_absent_policy_cleanup_only_removes_product_persistent_state() {
        assert!(stale_cleanup_commands(DnsPlatform::Linux, 7, "tun7").is_empty());
        for platform in [DnsPlatform::Macos, DnsPlatform::Windows] {
            assert_eq!(
                stale_cleanup_commands(platform, 7, "tun7"),
                cleanup_commands(platform, 7, "tun7")
            );
        }
    }

    #[test]
    fn global_command_count_and_size_are_bounded_without_domain_input() {
        for platform in [DnsPlatform::Linux, DnsPlatform::Macos, DnsPlatform::Windows] {
            let commands = apply_commands(
                platform,
                u32::MAX,
                "vpn-interface",
                Ipv4Addr::new(255, 255, 255, 254),
            );
            assert!(commands.len() <= 2);
            let size: usize = commands
                .iter()
                .map(|command| {
                    command.args.iter().map(String::len).sum::<usize>()
                        + command.stdin.as_ref().map_or(0, String::len)
                })
                .sum();
            assert!(size < 8192);
        }
    }

    #[test]
    fn linux_global_commands_are_scoped_to_link() {
        let global = apply_commands(DnsPlatform::Linux, 7, "tun7", "10.9.0.1".parse().unwrap());
        assert_eq!(global[1].args, ["domain", "tun7", "~."]);
        assert_eq!(global[0].args, ["dns", "tun7", "10.9.0.1"]);
        assert_eq!(
            cleanup_commands(DnsPlatform::Linux, 7, "tun7")[0].args,
            ["revert", "tun7"]
        );
    }

    #[test]
    fn macos_uses_only_product_owned_dynamic_store_key() {
        let commands = apply_commands(DnsPlatform::Macos, 4, "utun4", "10.9.0.1".parse().unwrap());
        let input = commands[0].stdin.as_deref().unwrap();
        assert!(input.contains("State:/Network/Service/com.xeflow.yilian.vpn/DNS"));
        assert!(input.contains("SupplementalMatchDomains * \"\""));
        let cleanup = cleanup_commands(DnsPlatform::Macos, 4, "utun4");
        assert_eq!(
            cleanup[0].stdin.as_deref(),
            Some("remove State:/Network/Service/com.xeflow.yilian.vpn/DNS\nquit\n")
        );
    }

    #[test]
    fn windows_nrpt_rules_have_owner_marker() {
        let commands = apply_commands(DnsPlatform::Windows, 12, "12", "10.9.0.1".parse().unwrap());
        let script = &commands[0].args[3];
        assert!(script.contains("Add-DnsClientNrptRule"));
        assert!(script.contains(OWNER));
        assert!(script.contains("-Namespace '.'"));
        assert!(!script.contains("Set-NetIPInterface"));
        assert!(!script.contains("InterfaceMetric"));
        assert_eq!(script.matches("Add-DnsClientNrptRule").count(), 1);
        assert!(script.len() < 8192);
        let cleanup = cleanup_commands(DnsPlatform::Windows, 12, "12");
        assert!(cleanup[0].args[3].contains(&format!("$_.Comment -eq '{OWNER}'")));
        assert!(cleanup[1].args[3].contains("-InterfaceIndex 12 -ResetServerAddresses"));
    }
    struct FakeDns {
        health: tokio::sync::mpsc::UnboundedReceiver<bool>,
        events: tokio::sync::mpsc::UnboundedSender<&'static str>,
        active: bool,
        fail_restore_once: bool,
    }

    #[async_trait::async_trait]
    impl DnsMaintenance for FakeDns {
        async fn probe(&mut self) -> Result<()> {
            self.events.send("probe").unwrap();
            match self.health.recv().await {
                Some(true) => Ok(()),
                _ => Err(PlatformError::command("probe", "unavailable")),
            }
        }
        async fn apply(&mut self) -> Result<()> {
            self.active = true;
            self.events.send("apply").unwrap();
            Ok(())
        }
        async fn restore(&mut self) -> Result<()> {
            if self.active {
                self.events.send("restore").unwrap();
                if self.fail_restore_once {
                    self.fail_restore_once = false;
                    return Err(PlatformError::command("restore", "temporary failure"));
                }
                self.active = false;
            }
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn dns_is_applied_only_after_probe_and_restored_on_failure_recovery_and_owner_loss() {
        let (health, rx) = tokio::sync::mpsc::unbounded_channel();
        let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
        let (stop, shutdown) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut backend = FakeDns {
                health: rx,
                events,
                active: false,
                fail_restore_once: false,
            };
            maintain_dns(&mut backend, shutdown).await
        });
        assert_eq!(log.recv().await, Some("probe"));
        health.send(false).unwrap();
        // An unavailable DNS never installs a system policy.
        assert_eq!(log.recv().await, Some("probe"));
        health.send(true).unwrap();
        assert_eq!(log.recv().await, Some("apply"));
        assert_eq!(log.recv().await, Some("probe"));
        health.send(false).unwrap();
        assert_eq!(log.recv().await, Some("restore"));
        assert_eq!(log.recv().await, Some("probe"));
        health.send(true).unwrap();
        assert_eq!(log.recv().await, Some("apply"));
        assert_eq!(log.recv().await, Some("probe"));
        // Cancel a pending network probe by dropping the forwarding task's sender.
        drop(stop);
        assert_eq!(log.recv().await, Some("restore"));
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn dns_restore_failure_is_retried_and_reported() {
        let (health, rx) = tokio::sync::mpsc::unbounded_channel();
        let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
        let (_stop, shutdown) = watch::channel(false);
        health.send(true).unwrap();
        health.send(false).unwrap();
        let mut backend = FakeDns {
            health: rx,
            events,
            active: false,
            fail_restore_once: true,
        };
        assert!(maintain_dns(&mut backend, shutdown).await.is_err());
        assert!(!backend.active);
        let mut observed = Vec::new();
        while let Ok(event) = log.try_recv() {
            observed.push(event);
        }
        assert_eq!(observed, ["probe", "apply", "probe", "restore", "restore"]);
    }

    #[tokio::test]
    async fn probe_checks_reply_identity_and_resolution_result() {
        use hickory_proto::rr::{rdata::NS, RData, Record};
        for case in 0..5 {
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let server = socket.local_addr().unwrap();
            let worker = tokio::spawn(async move {
                let mut buffer = [0; 512];
                let (size, source) = socket.recv_from(&mut buffer).await.unwrap();
                let query = Message::from_vec(&buffer[..size]).unwrap();
                let mut response = Message::response(query.id, query.op_code);
                response.add_queries(query.queries.clone());
                response.add_answer(Record::from_rdata(
                    Name::root(),
                    0,
                    RData::NS(NS(Name::from_ascii("a.root-servers.net.").unwrap())),
                ));
                match case {
                    1 => response.metadata.id = query.id.wrapping_add(1),
                    2 => response.metadata.response_code = ResponseCode::ServFail,
                    3 => response.queries.clear(),
                    4 => response.answers.clear(),
                    _ => {}
                }
                socket
                    .send_to(&response.to_vec().unwrap(), source)
                    .await
                    .unwrap();
            });
            assert_eq!(
                probe_dns(Ipv4Addr::LOCALHOST, server).await.is_ok(),
                case == 0
            );
            worker.await.unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn maintenance_command_timeout_is_bounded() {
        let result = run_commands(vec![spec("/bin/sleep", ["60"])]).await;
        assert!(result.unwrap_err().to_string().contains("超时"));
    }

    #[test]
    fn windows_lease_is_volatile_and_cleans_exact_rule_on_pipe_eof() {
        let script = windows_lease_script(12, Ipv4Addr::new(10, 9, 0, 1));
        assert!(script.contains("RegistryOptions]::Volatile"));
        assert!(script.contains("[Console]::In.ReadLine()"));
        assert!(script.contains("DeleteSubKey($rule.Name, $false)"));
        assert!(script.contains("yilian-dns-lease:{0}:{1}"));
        assert!(script.contains("Global\\com.xeflow.yilian.vpn.dns"));
        assert!(!script.contains("__IFINDEX__"));
        assert!(!script.contains("__SERVER__"));
        let cleanup = windows_policy_cleanup();
        assert!(cleanup.contains("StartTime.ToUniversalTime().Ticks"));
        assert!(cleanup.contains("if (-not $live)"));
        assert!(cleanup.contains("if (-not $hasRules) { return }"));
        assert!(!cleanup.contains("Remove-Item"));
    }
}
