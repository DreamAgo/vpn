//! 客户端配置与凭证持久化。
//!
//! 凭证（server_url + refresh_token）通过 vpn-platform 的 [`CredentialStore`]
//! 持久化，key 命名空间用 [`vpn_platform::DEFAULT_SERVICE`]（"vpn-cli"）。
//! 主路径 keyring，失败时降级到加密文件。

use std::path::PathBuf;

use vpn_platform::{CredentialStore, FileCredentialStore, KeyringCredentialStore};

use crate::error::{CliError, CliResult};

/// 凭证 key：服务端 URL。
pub const KEY_SERVER_URL: &str = "server_url";
/// 凭证 key：refresh token。
pub const KEY_REFRESH_TOKEN: &str = "refresh_token";
/// 凭证 key：登录用户名（可选，便于复用）。
pub const KEY_USERNAME: &str = "username";
/// 凭证 key：本机作为站点网关时声明的 LAN 网段（逗号分隔 CIDR）。
pub const KEY_ROUTES: &str = "routed_subnets";

/// 默认客户端 WireGuard 接口名（可经环境变量 `VPN_CLI_INTERFACE` 覆盖）。
pub const DEFAULT_INTERFACE: &str = "vpncli0";

/// daemon 运行配置。
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// 服务端基础 URL。
    pub server_url: String,
    /// 已保存的 refresh token（缺失则无法启动）。
    pub refresh_token: Option<String>,
    /// 设备名（register 上送）。
    pub device_name: String,
    /// IPC socket 路径。
    pub socket_path: PathBuf,
    /// 本机背后路由的 LAN 网段（站点网关模式，register 上送）。
    pub routed_subnets: Vec<String>,
    /// 客户端 WireGuard 接口名（内核数据面用）。
    pub interface: String,
}

/// 凭证仓库：封装底层 [`CredentialStore`]，提供 server_url / refresh_token 读写。
pub struct CredentialRepo {
    store: Box<dyn CredentialStore>,
}

impl CredentialRepo {
    /// 用给定后端构造。
    pub fn new(store: Box<dyn CredentialStore>) -> Self {
        Self { store }
    }

    /// 默认后端：优先 keyring；本函数不探测可用性（探测留给真机调用方）。
    pub fn keyring() -> Self {
        Self::new(Box::new(KeyringCredentialStore::new()))
    }

    /// 加密文件降级后端。
    pub fn file() -> CliResult<Self> {
        Ok(Self::new(Box::new(FileCredentialStore::new()?)))
    }

    /// 保存一次成功登录的凭证。
    pub fn save_login(
        &self,
        server_url: &str,
        refresh_token: &str,
        username: Option<&str>,
    ) -> CliResult<()> {
        self.store.save(KEY_SERVER_URL, server_url)?;
        self.store.save(KEY_REFRESH_TOKEN, refresh_token)?;
        if let Some(u) = username {
            self.store.save(KEY_USERNAME, u)?;
        }
        Ok(())
    }

    /// 保存站点网关路由网段（逗号分隔）。空则清除。
    pub fn save_routes(&self, routes: &[String]) -> CliResult<()> {
        if routes.is_empty() {
            let _ = self.store.delete(KEY_ROUTES);
        } else {
            self.store.save(KEY_ROUTES, &routes.join(","))?;
        }
        Ok(())
    }

    /// 读取已保存的路由网段。
    pub fn routes(&self) -> CliResult<Vec<String>> {
        Ok(self
            .store
            .load(KEY_ROUTES)?
            .map(|s| {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .map(|x| x.to_string())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 读取已保存的 server_url。
    pub fn server_url(&self) -> CliResult<Option<String>> {
        Ok(self.store.load(KEY_SERVER_URL)?)
    }

    /// 读取已保存的 refresh token。
    pub fn refresh_token(&self) -> CliResult<Option<String>> {
        Ok(self.store.load(KEY_REFRESH_TOKEN)?)
    }

    /// 读取已保存的用户名。
    pub fn username(&self) -> CliResult<Option<String>> {
        Ok(self.store.load(KEY_USERNAME)?)
    }

    /// 清除所有凭证（logout）。
    pub fn clear(&self) -> CliResult<()> {
        self.store.delete(KEY_REFRESH_TOKEN)?;
        self.store.delete(KEY_SERVER_URL)?;
        self.store.delete(KEY_USERNAME)?;
        let _ = self.store.delete(KEY_ROUTES);
        Ok(())
    }

    /// 组装 daemon 配置：要求已登录（有 server_url + refresh_token）。
    pub fn to_daemon_config(
        &self,
        device_name: String,
        socket_path: PathBuf,
    ) -> CliResult<DaemonConfig> {
        let server_url = self.server_url()?.ok_or(CliError::NotLoggedIn)?;
        let refresh_token = self.refresh_token()?;
        if refresh_token.is_none() {
            return Err(CliError::NotLoggedIn);
        }
        Ok(DaemonConfig {
            server_url,
            refresh_token,
            device_name,
            socket_path,
            routed_subnets: self.routes()?,
            interface: std::env::var("VPN_CLI_INTERFACE")
                .unwrap_or_else(|_| DEFAULT_INTERFACE.to_string()),
        })
    }
}

/// 默认设备名：主机名（不可得时回退到 "vpn-cli-device"）。
pub fn default_device_name() -> String {
    hostname().unwrap_or_else(|| "vpn-cli-device".to_string())
}

fn hostname() -> Option<String> {
    // 优先环境变量（跨平台、可测），否则留空。
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn file_repo(dir: &std::path::Path) -> CredentialRepo {
        let path = dir.join("creds.enc");
        let store = FileCredentialStore::with_path_and_home(path, dir);
        CredentialRepo::new(Box::new(store))
    }

    #[test]
    fn save_and_read_back() {
        let dir = tempdir().unwrap();
        let repo = file_repo(dir.path());
        assert_eq!(repo.server_url().unwrap(), None);
        assert_eq!(repo.refresh_token().unwrap(), None);

        repo.save_login("https://vpn.example.com", "rtk-123", Some("alice"))
            .unwrap();
        assert_eq!(
            repo.server_url().unwrap(),
            Some("https://vpn.example.com".to_string())
        );
        assert_eq!(repo.refresh_token().unwrap(), Some("rtk-123".to_string()));
        assert_eq!(repo.username().unwrap(), Some("alice".to_string()));
    }

    #[test]
    fn clear_removes_all() {
        let dir = tempdir().unwrap();
        let repo = file_repo(dir.path());
        repo.save_login("u", "r", Some("n")).unwrap();
        repo.clear().unwrap();
        assert_eq!(repo.server_url().unwrap(), None);
        assert_eq!(repo.refresh_token().unwrap(), None);
        assert_eq!(repo.username().unwrap(), None);
    }

    #[test]
    fn to_daemon_config_requires_login() {
        let dir = tempdir().unwrap();
        let repo = file_repo(dir.path());
        // 未登录
        let err = repo
            .to_daemon_config("dev".into(), PathBuf::from("/tmp/x.sock"))
            .unwrap_err();
        assert!(matches!(err, CliError::NotLoggedIn));

        repo.save_login("https://s", "rtk", None).unwrap();
        let cfg = repo
            .to_daemon_config("dev".into(), PathBuf::from("/tmp/x.sock"))
            .unwrap();
        assert_eq!(cfg.server_url, "https://s");
        assert_eq!(cfg.refresh_token, Some("rtk".to_string()));
        assert_eq!(cfg.device_name, "dev");
    }

    #[test]
    fn default_device_name_nonempty() {
        assert!(!default_device_name().is_empty());
    }
}
