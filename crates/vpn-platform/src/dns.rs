//! 在产品自有 TUN 接口上应用、恢复服务端下发的 DNS 策略。

use std::net::Ipv4Addr;

use tokio::{io::AsyncWriteExt, process::Command};
use vpn_api_types::{peer::ClientDnsSettings, system::ClientDnsMode};

use crate::{PlatformError, Result};

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
}

impl DnsSession {
    pub async fn restore(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
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
    let commands = apply_commands(platform, ifindex, &interface_name, server);
    if let Err(error) = run_commands(commands).await {
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
    }))
}

/// 清理上次进程异常退出可能遗留的持久产品状态。Linux link DNS 随 TUN 消失，无需处理。
pub async fn cleanup_stale_dns(ifindex: u32) -> Result<()> {
    let platform = current_platform()?;
    let interface_name = interface_name(ifindex)?;
    run_commands(stale_cleanup_commands(platform, ifindex, &interface_name)).await
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
        if spec.stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(input) = spec.stdin {
            child
                .stdin
                .take()
                .ok_or_else(|| PlatformError::command(spec.program, "无法打开 stdin"))?
                .write_all(input.as_bytes())
                .await?;
        }
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(PlatformError::command(
                spec.program,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
    }
    Ok(())
}

fn cleanup_commands(platform: DnsPlatform, ifindex: u32, interface: &str) -> Vec<CommandSpec> {
    match platform {
        DnsPlatform::Linux => vec![spec("resolvectl", ["revert", interface])],
        DnsPlatform::Macos => vec![CommandSpec {
            program: "/usr/sbin/scutil",
            args: vec![],
            stdin: Some(format!("remove State:/Network/Service/{OWNER}/DNS\nquit\n")),
        }],
        DnsPlatform::Windows => vec![powershell(format!(
            "$ErrorActionPreference='Stop'; Get-DnsClientNrptRule | Where-Object {{ $_.Comment -eq '{OWNER}' }} | Remove-DnsClientNrptRule -Force; if (Get-NetAdapter -InterfaceIndex {ifindex} -ErrorAction SilentlyContinue) {{ Set-DnsClientServerAddress -InterfaceIndex {ifindex} -ResetServerAddresses }}"
        ))],
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
                program: "/usr/sbin/scutil",
                args: vec![],
                stdin: Some(format!(
                    "d.init\nd.add ServerAddresses * {server}\nd.add InterfaceName {interface}\nd.add SearchOrder # 1\nd.add SupplementalMatchDomains * \"\"\nd.add SupplementalMatchDomainsNoSearch # 1\nset State:/Network/Service/{OWNER}/DNS\nquit\n"
                )),
            }],
        DnsPlatform::Windows => vec![powershell(format!(
            "$ErrorActionPreference='Stop'; Set-DnsClientServerAddress -InterfaceIndex {ifindex} -ServerAddresses '{server}'; Add-DnsClientNrptRule -Namespace '.' -NameServers '{server}' -Comment '{OWNER}'"
        ))],
    }
}

fn spec<const N: usize>(program: &'static str, args: [&str; N]) -> CommandSpec {
    CommandSpec {
        program,
        args: args.into_iter().map(str::to_string).collect(),
        stdin: None,
    }
}

fn powershell(script: String) -> CommandSpec {
    CommandSpec {
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
            assert!(size < 1024);
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
        assert!(script.len() < 1024);
        let cleanup = cleanup_commands(DnsPlatform::Windows, 12, "12");
        assert!(cleanup[0].args[3].contains(&format!("$_.Comment -eq '{OWNER}'")));
        assert!(cleanup[0].args[3].contains("-InterfaceIndex 12 -ResetServerAddresses"));
    }
}
