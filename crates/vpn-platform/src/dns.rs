//! 在产品自有 TUN 接口上应用、恢复服务端下发的 DNS 策略。

use std::net::Ipv4Addr;

use tokio::{io::AsyncWriteExt, process::Command};
use vpn_api_types::{
    peer::ClientDnsSettings,
    system::{normalize_dns_domain, ClientDnsMode},
};

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
        return Ok(None);
    }
    let server: Ipv4Addr = settings.server.parse().map_err(|_| {
        PlatformError::InvalidArgument(format!("服务端下发了非法 DNS 地址：{}", settings.server))
    })?;
    let domains = settings
        .domains
        .iter()
        .map(|domain| normalize_dns_domain(domain).map_err(PlatformError::InvalidArgument))
        .collect::<Result<Vec<_>>>()?;
    if settings.mode == ClientDnsMode::Split && domains.is_empty() {
        return Err(PlatformError::InvalidArgument(
            "分流 DNS 缺少域名".to_string(),
        ));
    }
    let platform = current_platform()?;
    let interface_name = interface_name(ifindex)?;
    run_commands(cleanup_commands(platform, ifindex, &interface_name)).await?;
    let commands = apply_commands(
        platform,
        ifindex,
        &interface_name,
        server,
        settings.mode,
        &domains,
    );
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
    tracing::info!(stage = "dns_apply", result = "succeeded", ?settings.mode, domains = domains.len(), "客户端 DNS 已应用");
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
    let commands = match platform {
        DnsPlatform::Linux => Vec::new(),
        _ => cleanup_commands(platform, ifindex, &interface_name),
    };
    run_commands(commands).await
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
    mode: ClientDnsMode,
    domains: &[String],
) -> Vec<CommandSpec> {
    match platform {
        DnsPlatform::Linux => {
            let mut commands = vec![spec("resolvectl", ["dns", interface, &server.to_string()])];
            let routing_domains = if mode == ClientDnsMode::Global {
                vec!["~.".to_string()]
            } else {
                domains.iter().map(|domain| format!("~{domain}")).collect()
            };
            let mut args = vec!["domain".to_string(), interface.to_string()];
            args.extend(routing_domains);
            commands.push(CommandSpec {
                program: "resolvectl",
                args,
                stdin: None,
            });
            commands
        }
        DnsPlatform::Macos => {
            let supplemental_domains = if mode == ClientDnsMode::Split {
                domains.join(" ")
            } else {
                // Apple VPN DNS 语义：空 match domain 把分隧道 DNS 提升为默认解析器。
                "\"\"".to_string()
            };
            vec![CommandSpec {
                program: "/usr/sbin/scutil",
                args: vec![],
                stdin: Some(format!(
                    "d.init\nd.add ServerAddresses * {server}\nd.add InterfaceName {interface}\nd.add SearchOrder # 1\nd.add SupplementalMatchDomains * {supplemental_domains}\nd.add SupplementalMatchDomainsNoSearch # 1\nset State:/Network/Service/{OWNER}/DNS\nquit\n"
                )),
            }]
        }
        DnsPlatform::Windows => {
            let mut script = "$ErrorActionPreference='Stop'".to_string();
            if mode == ClientDnsMode::Global {
                script.push_str(&format!(
                    "; Set-DnsClientServerAddress -InterfaceIndex {ifindex} -ServerAddresses '{server}'; Set-NetIPInterface -InterfaceIndex {ifindex} -InterfaceMetric 1; Add-DnsClientNrptRule -Namespace '.' -NameServers '{server}' -Comment '{OWNER}'"
                ));
            } else {
                script.push_str(&format!(
                    "; Set-DnsClientServerAddress -InterfaceIndex {ifindex} -ResetServerAddresses"
                ));
                for domain in domains {
                    script.push_str(&format!(
                        "; Add-DnsClientNrptRule -Namespace '.{domain}' -NameServers '{server}' -Comment '{OWNER}'"
                    ));
                }
            }
            vec![powershell(script)]
        }
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
    fn linux_global_and_split_commands_are_scoped_to_link() {
        let global = apply_commands(
            DnsPlatform::Linux,
            7,
            "tun7",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Global,
            &[],
        );
        assert_eq!(global[1].args, ["domain", "tun7", "~."]);
        let split = apply_commands(
            DnsPlatform::Linux,
            7,
            "tun7",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Split,
            &["corp.example.com".into()],
        );
        assert_eq!(split[1].args, ["domain", "tun7", "~corp.example.com"]);
    }

    #[test]
    fn macos_uses_only_product_owned_dynamic_store_key() {
        let commands = apply_commands(
            DnsPlatform::Macos,
            4,
            "utun4",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Split,
            &["corp.example.com".into()],
        );
        let input = commands[0].stdin.as_deref().unwrap();
        assert!(input.contains("State:/Network/Service/com.xeflow.yilian.vpn/DNS"));
        assert!(input.contains("SupplementalMatchDomains"));
        let global = apply_commands(
            DnsPlatform::Macos,
            4,
            "utun4",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Global,
            &[],
        );
        assert!(global[0]
            .stdin
            .as_deref()
            .unwrap()
            .contains("SupplementalMatchDomains * \"\""));
    }

    #[test]
    fn windows_nrpt_rules_have_owner_marker() {
        let commands = apply_commands(
            DnsPlatform::Windows,
            12,
            "12",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Split,
            &["corp.example.com".into()],
        );
        let script = &commands[0].args[3];
        assert!(script.contains("Add-DnsClientNrptRule"));
        assert!(script.contains(OWNER));
        let global = apply_commands(
            DnsPlatform::Windows,
            12,
            "12",
            "10.9.0.1".parse().unwrap(),
            ClientDnsMode::Global,
            &[],
        );
        assert!(global[0].args[3].contains("-Namespace '.'"));
    }
}
