//! Linux kernel WireGuard 的 nftables 强制访问控制。
//!
//! 规则仅挂在指定 WireGuard 接口的 FORWARD 路径；SQLite 是事实源，这里只维护
//! 带短租约的派生快照。进程停止刷新后，授权元素会自动超时，最终 fail-closed。

use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv4Addr,
    process::Stdio,
};

use ipnet::Ipv4Net;
use tokio::{io::AsyncWriteExt, process::Command};
use vpn_core::{AppError, Result};

const TABLE_FAMILY: &str = "inet";
const TABLE_NAME: &str = "yilian_vpn_acl";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclLease {
    pub source: Ipv4Addr,
    pub destination: Ipv4Net,
    /// 从生成快照开始计算的剩余秒数，调用方必须限制在短租约上限内。
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct NftAclController {
    iface: String,
    vpn_subnet: Ipv4Net,
}

impl NftAclController {
    pub fn new(iface: impl Into<String>, vpn_subnet: Ipv4Net) -> Result<Self> {
        let iface = iface.into();
        if iface.is_empty()
            || !iface
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(AppError::Config("WireGuard 接口名不能用于 nft ACL".into()));
        }
        Ok(Self { iface, vpn_subnet })
    }

    /// 生产 kernel 后端启动前探测 nft。任何失败都直接返回，禁止软降级。
    pub async fn verify_available(&self) -> Result<()> {
        let output = Command::new("nft")
            .arg("--version")
            .output()
            .await
            .map_err(|error| AppError::WireGuard(format!("nft 不可用: {error}")))?;
        if !output.status.success() {
            return Err(AppError::WireGuard(format!(
                "nft 不可用: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// 审批 ACL 被关闭时清理本组件独占的 table，避免固定 final-drop 在容器/网络命名空间
    /// 复用后残留。未安装 nft 视为没有可清理状态；其他探测/删除错误必须上抛。
    pub async fn cleanup_owned_table_if_present() -> Result<()> {
        let output = match Command::new("nft")
            .args(["list", "table", TABLE_FAMILY, TABLE_NAME])
            .output()
            .await
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(AppError::WireGuard(format!(
                    "探测遗留 nft ACL table 失败: {error}"
                )))
            }
        };
        if output.status.success() {
            return run_nft_batch(
                &format!("delete table {TABLE_FAMILY} {TABLE_NAME}\n"),
                false,
            )
            .await;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such file or directory") || stderr.contains("does not exist") {
            Ok(())
        } else {
            Err(AppError::WireGuard(format!(
                "探测遗留 nft ACL table 失败: {}",
                stderr.trim()
            )))
        }
    }

    /// 校验后原子更新独立 table。若 table 尚不存在，首次事务会连同默认 drop 链一起创建。
    pub async fn apply(&self, leases: &[AclLease], site_sources: &[Ipv4Net]) -> Result<()> {
        let started = std::time::Instant::now();
        let exists = Command::new("nft")
            .args(["list", "table", TABLE_FAMILY, TABLE_NAME])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|error| AppError::WireGuard(format!("探测 nft ACL table 失败: {error}")))?
            .success();
        let batch = render_nft_batch(&self.iface, self.vpn_subnet, leases, site_sources, exists);
        run_nft_batch(&batch, true).await?;
        // `nft --check` 也会消耗租约时间；实际事务必须使用扣减后的 timeout，不能
        // 让校验时延把授权推过数据库中的独占 expires_at。
        let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let adjusted = leases
            .iter()
            .filter_map(|lease| {
                let timeout_ms = lease.timeout_ms.saturating_sub(elapsed_ms);
                (timeout_ms > 0).then_some(AclLease {
                    source: lease.source,
                    destination: lease.destination,
                    timeout_ms,
                })
            })
            .collect::<Vec<_>>();
        let batch = render_nft_batch(
            &self.iface,
            self.vpn_subnet,
            &adjusted,
            site_sources,
            exists,
        );
        run_nft_batch(&batch, false).await
    }
}

async fn run_nft_batch(batch: &str, check_only: bool) -> Result<()> {
    let mut command = Command::new("nft");
    if check_only {
        command.arg("--check");
    }
    command.kill_on_drop(true);
    let mut child = command
        .args(["--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::WireGuard(format!("启动 nft 失败: {error}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| AppError::WireGuard("nft stdin 不可用".into()))?
        .write_all(batch.as_bytes())
        .await
        .map_err(|error| AppError::WireGuard(format!("写入 nft 规则失败: {error}")))?;
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
        .await
        .map_err(|_| AppError::WireGuard("nft ACL 命令超时".into()))?
        .map_err(|error| AppError::WireGuard(format!("等待 nft 失败: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::WireGuard(format!(
            "nft ACL {}失败: {}",
            if check_only { "校验" } else { "更新" },
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

pub fn render_nft_batch(
    iface: &str,
    vpn_subnet: Ipv4Net,
    leases: &[AclLease],
    site_sources: &[Ipv4Net],
    table_exists: bool,
) -> String {
    let mut script = String::new();
    if table_exists {
        // delete + recreate 位于同一 nft transaction，不产生中间放行窗口，也不依赖
        // 不同 nft 版本对 `flush table` 是否保留 chain/set 对象的差异。
        script.push_str(&format!("delete table {TABLE_FAMILY} {TABLE_NAME}\n"));
    }
    script.push_str(&format!("add table {TABLE_FAMILY} {TABLE_NAME}\n"));
    script.push_str(&format!(
        "add chain {TABLE_FAMILY} {TABLE_NAME} forward {{ type filter hook forward priority -100; policy accept; }}\n"
    ));
    // VPN 内基础通信始终允许。该规则不允许访问任意业务网段。
    script.push_str(&format!(
        "add rule {TABLE_FAMILY} {TABLE_NAME} forward iifname \"{iface}\" ip daddr {vpn_subnet} accept\n"
    ));

    let mut sites = BTreeSet::new();
    sites.extend(site_sources.iter().copied());
    for source in sites {
        script.push_str(&format!(
            "add rule {TABLE_FAMILY} {TABLE_NAME} forward iifname \"{iface}\" ip saddr {source} ip daddr {vpn_subnet} accept\n"
        ));
    }

    // 同一目的网段共用一个带 timeout 的源地址集合；同一授权取最长剩余时间。
    let mut routes: BTreeMap<Ipv4Net, BTreeMap<Ipv4Addr, u64>> = BTreeMap::new();
    for lease in leases.iter().filter(|lease| lease.timeout_ms > 0) {
        routes
            .entry(lease.destination)
            .or_default()
            .entry(lease.source)
            .and_modify(|timeout| *timeout = (*timeout).max(lease.timeout_ms))
            .or_insert(lease.timeout_ms);
    }
    for (index, (destination, sources)) in routes.into_iter().enumerate() {
        let set_name = format!("route_{index}");
        let max_timeout = sources.values().copied().max().unwrap_or(1).max(1);
        script.push_str(&format!(
            "add set {TABLE_FAMILY} {TABLE_NAME} {set_name} {{ type ipv4_addr; flags timeout; timeout {max_timeout}ms; }}\n"
        ));
        let elements = sources
            .into_iter()
            .map(|(source, timeout)| format!("{source} timeout {}ms", timeout.max(1)))
            .collect::<Vec<_>>()
            .join(", ");
        script.push_str(&format!(
            "add element {TABLE_FAMILY} {TABLE_NAME} {set_name} {{ {elements} }}\n"
        ));
        script.push_str(&format!(
            "add rule {TABLE_FAMILY} {TABLE_NAME} forward iifname \"{iface}\" ip saddr @{set_name} ip daddr {destination} accept\n"
        ));
    }
    script.push_str(&format!(
        "add rule {TABLE_FAMILY} {TABLE_NAME} forward iifname \"{iface}\" counter drop\n"
    ));
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fail_closed_forward_table_with_exact_timeouts() {
        let script = render_nft_batch(
            "wg0",
            "10.8.0.0/24".parse().unwrap(),
            &[
                AclLease {
                    source: "10.8.0.2".parse().unwrap(),
                    destination: "192.168.1.0/24".parse().unwrap(),
                    timeout_ms: 17_000,
                },
                AclLease {
                    source: "10.8.0.3".parse().unwrap(),
                    destination: "192.168.1.0/24".parse().unwrap(),
                    timeout_ms: 90_000,
                },
            ],
            &["172.16.1.0/24".parse().unwrap()],
            false,
        );
        assert!(script.contains("add table inet yilian_vpn_acl"));
        assert!(script.contains("hook forward priority -100; policy accept"));
        assert!(script.contains("ip daddr 10.8.0.0/24 accept"));
        assert!(script.contains("10.8.0.2 timeout 17000ms"));
        assert!(script.contains("10.8.0.3 timeout 90000ms"));
        assert!(script.contains("ip saddr 172.16.1.0/24 ip daddr 10.8.0.0/24 accept"));
        assert!(script.ends_with("iifname \"wg0\" counter drop\n"));
    }

    #[test]
    fn update_recreates_only_owned_table_atomically() {
        let script = render_nft_batch("wg0", "10.8.0.0/24".parse().unwrap(), &[], &[], true);
        assert!(script.starts_with("delete table inet yilian_vpn_acl\n"));
        assert!(script.contains("add table inet yilian_vpn_acl"));
        assert!(script.contains("counter drop"));
    }
}
