//! Story 4.10: 跨平台 Daemon / 服务运行时抽象。
//!
//! 设计：
//! - [`DaemonRuntime`] trait 抽象 install / uninstall / start / stop / status。
//! - 三平台后端：
//!   - Linux：systemd **user** service（`~/.config/systemd/user/vpn-cli.service`
//!     + `systemctl --user`）。
//!   - macOS：launchd LaunchAgent（`~/Library/LaunchAgents/<label>.plist`
//!     + `launchctl`）。
//!   - Windows：Windows Service（`sc.exe`）。
//! - **可测纯逻辑**（本机单测）：
//!   - systemd unit 文件内容渲染 [`render_systemd_unit`]。
//!   - launchd plist 内容渲染 [`render_launchd_plist`]。
//!   - 服务文件路径推导（各后端 `*_path` 函数）。
//! - **需真机验证**：真正调用 `systemctl` / `launchctl` / `sc.exe`
//!   与系统服务管理器交互的部分留实现骨架（`std::process::Command`），
//!   在 CI / 真机执行。

use std::path::{Path, PathBuf};

use crate::error::{PlatformError, Result};

/// 默认服务标识 / 名称。
pub const DEFAULT_SERVICE_NAME: &str = "vpn-cli";
/// launchd label 约定。
pub const DEFAULT_LAUNCHD_LABEL: &str = "com.xeflow.vpn.cli.daemon";

/// 服务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    /// 已安装且正在运行。
    Running,
    /// 已安装但未运行。
    Stopped,
    /// 未安装。
    NotInstalled,
}

/// 跨平台 daemon / 服务运行时 trait。
pub trait DaemonRuntime {
    /// 安装服务：写服务描述文件并向服务管理器注册。
    fn install(&self, exec_path: &Path, args: &[String]) -> Result<()>;
    /// 卸载服务：停止并移除注册与描述文件。
    fn uninstall(&self) -> Result<()>;
    /// 启动服务。
    fn start(&self) -> Result<()>;
    /// 停止服务。
    fn stop(&self) -> Result<()>;
    /// 查询服务状态。
    fn status(&self) -> Result<DaemonStatus>;
}

// ===========================================================================
// 纯逻辑：服务描述文件渲染（三平台均可在任意主机上单测）
// ===========================================================================

/// 渲染 systemd **user** service unit 文件内容。
///
/// `exec_path` 为可执行文件绝对路径，`args` 为附加参数（会被空格拼接到 ExecStart）。
pub fn render_systemd_unit(description: &str, exec_path: &Path, args: &[String]) -> String {
    let exec_start = build_exec_start(exec_path, args);
    format!(
        "[Unit]\n\
         Description={description}\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_start}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// 渲染 launchd LaunchAgent plist 内容。
pub fn render_launchd_plist(label: &str, exec_path: &Path, args: &[String]) -> String {
    let mut program_args = String::new();
    program_args.push_str(&format!(
        "\t\t<string>{}</string>\n",
        xml_escape(&exec_path.to_string_lossy())
    ));
    for a in args {
        program_args.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(a)));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n{program_args}\t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// 拼接 ExecStart 命令行（含可执行路径 + 参数）。
fn build_exec_start(exec_path: &Path, args: &[String]) -> String {
    let mut s = exec_path.to_string_lossy().into_owned();
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// 最小 XML 转义（用于 plist 中的字符串字段）。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// --- 路径推导（纯逻辑，可单测）-------------------------------------------

/// systemd user unit 文件路径：`<config_dir>/systemd/user/<name>.service`。
pub fn systemd_unit_path(config_dir: &Path, service_name: &str) -> PathBuf {
    config_dir
        .join("systemd")
        .join("user")
        .join(format!("{service_name}.service"))
}

/// launchd LaunchAgent plist 路径：`<home>/Library/LaunchAgents/<label>.plist`。
pub fn launchd_plist_path(home: &Path, label: &str) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist"))
}

// ===========================================================================
// 平台后端
// ===========================================================================

/// 按当前平台返回默认的 [`DaemonRuntime`] 实现。
pub fn default_runtime() -> Result<Box<dyn DaemonRuntime>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::SystemdUserRuntime::new(
            DEFAULT_SERVICE_NAME,
        )?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::LaunchdRuntime::new(DEFAULT_LAUNCHD_LABEL)?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsServiceRuntime::new(
            DEFAULT_SERVICE_NAME,
        )))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(PlatformError::Unsupported(
            "daemon runtime 仅支持 linux/macos/windows".to_string(),
        ))
    }
}

/// 运行一个系统命令，非零退出码转为 [`PlatformError::Command`]。
///
/// 各后端的 install/start/... 复用此辅助；真实行为需真机验证。
#[allow(dead_code)]
fn run_command(program: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| PlatformError::command(program, format!("无法执行: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PlatformError::command(
            program,
            format!("退出码 {:?}: {}", output.status.code(), stderr.trim()),
        ));
    }
    Ok(output)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// systemd user service 后端。
    pub struct SystemdUserRuntime {
        service_name: String,
        unit_path: PathBuf,
    }

    impl SystemdUserRuntime {
        pub fn new(service_name: &str) -> Result<Self> {
            let config_dir = dirs::config_dir()
                .ok_or_else(|| PlatformError::Daemon("无法定位用户配置目录".to_string()))?;
            Ok(Self {
                service_name: service_name.to_string(),
                unit_path: systemd_unit_path(&config_dir, service_name),
            })
        }

        fn unit_arg(&self) -> String {
            format!("{}.service", self.service_name)
        }
    }

    impl DaemonRuntime for SystemdUserRuntime {
        fn install(&self, exec_path: &Path, args: &[String]) -> Result<()> {
            if let Some(parent) = self.unit_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let unit = render_systemd_unit("VPN CLI daemon", exec_path, args);
            std::fs::write(&self.unit_path, unit)?;
            // 真机验证：重载 systemd 用户实例并 enable。
            run_command("systemctl", &["--user", "daemon-reload"])?;
            run_command("systemctl", &["--user", "enable", &self.unit_arg()])?;
            Ok(())
        }

        fn uninstall(&self) -> Result<()> {
            let _ = self.stop();
            let _ = run_command("systemctl", &["--user", "disable", &self.unit_arg()]);
            if self.unit_path.exists() {
                std::fs::remove_file(&self.unit_path)?;
            }
            let _ = run_command("systemctl", &["--user", "daemon-reload"]);
            Ok(())
        }

        fn start(&self) -> Result<()> {
            run_command("systemctl", &["--user", "start", &self.unit_arg()])?;
            Ok(())
        }

        fn stop(&self) -> Result<()> {
            run_command("systemctl", &["--user", "stop", &self.unit_arg()])?;
            Ok(())
        }

        fn status(&self) -> Result<DaemonStatus> {
            if !self.unit_path.exists() {
                return Ok(DaemonStatus::NotInstalled);
            }
            // `is-active` 退出码 0 表示运行中。
            let output = std::process::Command::new("systemctl")
                .args(["--user", "is-active", &self.unit_arg()])
                .output()
                .map_err(|e| PlatformError::command("systemctl", format!("无法执行: {e}")))?;
            if output.status.success() {
                Ok(DaemonStatus::Running)
            } else {
                Ok(DaemonStatus::Stopped)
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    /// launchd LaunchAgent 后端。
    pub struct LaunchdRuntime {
        label: String,
        plist_path: PathBuf,
    }

    impl LaunchdRuntime {
        pub fn new(label: &str) -> Result<Self> {
            let home = dirs::home_dir()
                .ok_or_else(|| PlatformError::Daemon("无法定位用户主目录".to_string()))?;
            Ok(Self {
                label: label.to_string(),
                plist_path: launchd_plist_path(&home, label),
            })
        }
    }

    impl DaemonRuntime for LaunchdRuntime {
        fn install(&self, exec_path: &Path, args: &[String]) -> Result<()> {
            if let Some(parent) = self.plist_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let plist = render_launchd_plist(&self.label, exec_path, args);
            std::fs::write(&self.plist_path, plist)?;
            // 真机验证：load 到当前用户的 launchd domain。
            run_command(
                "launchctl",
                &["load", "-w", &self.plist_path.to_string_lossy()],
            )?;
            Ok(())
        }

        fn uninstall(&self) -> Result<()> {
            if self.plist_path.exists() {
                let _ = run_command(
                    "launchctl",
                    &["unload", "-w", &self.plist_path.to_string_lossy()],
                );
                std::fs::remove_file(&self.plist_path)?;
            }
            Ok(())
        }

        fn start(&self) -> Result<()> {
            run_command("launchctl", &["start", &self.label])?;
            Ok(())
        }

        fn stop(&self) -> Result<()> {
            run_command("launchctl", &["stop", &self.label])?;
            Ok(())
        }

        fn status(&self) -> Result<DaemonStatus> {
            if !self.plist_path.exists() {
                return Ok(DaemonStatus::NotInstalled);
            }
            // `launchctl list <label>` 退出码 0 表示已加载。
            let output = std::process::Command::new("launchctl")
                .args(["list", &self.label])
                .output()
                .map_err(|e| PlatformError::command("launchctl", format!("无法执行: {e}")))?;
            if output.status.success() {
                Ok(DaemonStatus::Running)
            } else {
                Ok(DaemonStatus::Stopped)
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    /// Windows Service 后端（通过 `sc.exe`）。
    pub struct WindowsServiceRuntime {
        service_name: String,
    }

    impl WindowsServiceRuntime {
        pub fn new(service_name: &str) -> Self {
            Self {
                service_name: service_name.to_string(),
            }
        }
    }

    impl DaemonRuntime for WindowsServiceRuntime {
        fn install(&self, exec_path: &Path, args: &[String]) -> Result<()> {
            // sc create <name> binPath= "<exe> <args>" start= auto
            // 真机验证：需要管理员权限。
            let mut bin = exec_path.to_string_lossy().into_owned();
            for a in args {
                bin.push(' ');
                bin.push_str(a);
            }
            let bin_path_arg = format!("binPath= {bin}");
            run_command(
                "sc",
                &[
                    "create",
                    &self.service_name,
                    &bin_path_arg,
                    "start=",
                    "auto",
                ],
            )?;
            Ok(())
        }

        fn uninstall(&self) -> Result<()> {
            let _ = self.stop();
            run_command("sc", &["delete", &self.service_name])?;
            Ok(())
        }

        fn start(&self) -> Result<()> {
            run_command("sc", &["start", &self.service_name])?;
            Ok(())
        }

        fn stop(&self) -> Result<()> {
            run_command("sc", &["stop", &self.service_name])?;
            Ok(())
        }

        fn status(&self) -> Result<DaemonStatus> {
            let output = std::process::Command::new("sc")
                .args(["query", &self.service_name])
                .output()
                .map_err(|e| PlatformError::command("sc", format!("无法执行: {e}")))?;
            if !output.status.success() {
                return Ok(DaemonStatus::NotInstalled);
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("RUNNING") {
                Ok(DaemonStatus::Running)
            } else {
                Ok(DaemonStatus::Stopped)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_contains_exec_and_sections() {
        let unit = render_systemd_unit(
            "VPN CLI daemon",
            Path::new("/usr/local/bin/vpn-cli"),
            &["daemon".to_string(), "--config=/etc/vpn.toml".to_string()],
        );
        assert!(unit.contains("[Unit]"));
        assert!(unit.contains("[Service]"));
        assert!(unit.contains("[Install]"));
        assert!(unit.contains("Description=VPN CLI daemon"));
        assert!(unit.contains("ExecStart=/usr/local/bin/vpn-cli daemon --config=/etc/vpn.toml"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn systemd_unit_without_args() {
        let unit = render_systemd_unit("d", Path::new("/bin/x"), &[]);
        assert!(unit.contains("ExecStart=/bin/x\n"));
    }

    #[test]
    fn launchd_plist_is_well_formed() {
        let plist = render_launchd_plist(
            "com.xeflow.vpn.cli.daemon",
            Path::new("/usr/local/bin/vpn-cli"),
            &["daemon".to_string()],
        );
        assert!(plist.contains("<?xml version=\"1.0\""));
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("<string>com.xeflow.vpn.cli.daemon</string>"));
        assert!(plist.contains("<string>/usr/local/bin/vpn-cli</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("</plist>"));
        // 标签闭合数量基本平衡。
        assert_eq!(
            plist.matches("<dict>").count(),
            plist.matches("</dict>").count()
        );
        assert_eq!(
            plist.matches("<array>").count(),
            plist.matches("</array>").count()
        );
    }

    #[test]
    fn launchd_plist_xml_escapes_args() {
        let plist = render_launchd_plist(
            "lbl",
            Path::new("/bin/x"),
            &["--filter=a<b&c>d".to_string()],
        );
        assert!(plist.contains("--filter=a&lt;b&amp;c&gt;d"));
        assert!(!plist.contains("a<b&c>d"));
    }

    #[test]
    fn systemd_path_derivation() {
        let p = systemd_unit_path(Path::new("/home/u/.config"), "vpn-cli");
        assert_eq!(
            p,
            PathBuf::from("/home/u/.config/systemd/user/vpn-cli.service")
        );
    }

    #[test]
    fn launchd_path_derivation() {
        let p = launchd_plist_path(Path::new("/Users/u"), "com.xeflow.vpn.cli.daemon");
        assert_eq!(
            p,
            PathBuf::from("/Users/u/Library/LaunchAgents/com.xeflow.vpn.cli.daemon.plist")
        );
    }
}
