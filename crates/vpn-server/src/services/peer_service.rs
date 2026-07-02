//! Peer 数据平面业务服务（Epic 4：注册 / 心跳 / 注销 / 配置下载 + 离线检测）。
//!
//! 持有服务端 WireGuard 状态：
//! - 服务端公钥（注册响应与客户端配置需要）
//! - `IpPool`（可变共享，置于 `tokio::sync::Mutex` 之后）
//! - `Arc<dyn WireGuardControl>`（本轮注入 Noop，真实后端留待真机集成）

use std::net::Ipv4Addr;
use std::sync::Arc;

use ipnet::Ipv4Net;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use vpn_api_types::peer::{PeerRegisterRequest, PeerRegisterResponse};
use vpn_core::{AppError, Result};
use vpn_wireguard::{
    generate_keypair, public_key_from_private, render_client_config, IpPool,
    KernelWireGuardControl, NoopWireGuardControl, WgMode, WgPeerConfig, WireGuardControl,
};

use crate::repositories::{
    peer_repo_sqlite::SqlitePeerRepository, system_config_repo_sqlite::SqliteSystemConfigRepository,
    user_group_repo_sqlite::SqliteUserGroupRepository,
};

/// system_config 中存储服务端 WG 私钥/公钥的 key。
pub const KEY_SERVER_WG_PRIVATE: &str = "server_wg_private_key";
pub const KEY_SERVER_WG_PUBLIC: &str = "server_wg_public_key";
/// system_config 中存储服务端 LAN 网段（逗号分隔 CIDR）的 key。
/// 运行时可经 admin 接口修改；存在则覆盖启动时的 `VPN_SERVER_ROUTES` 默认值。
pub const KEY_SERVER_ROUTES: &str = "server_routes";

/// 客户端配置下载里 PrivateKey 字段的占位符（服务端不持有客户端私钥）。
const CLIENT_PRIVATE_KEY_PLACEHOLDER: &str = "<在此填入客户端私钥>";
/// PersistentKeepalive 秒数（穿越 NAT）。
const PERSISTENT_KEEPALIVE: u16 = 25;
/// 离线判定阈值：心跳超过该毫秒数未更新视为离线。
pub const OFFLINE_THRESHOLD_MS: i64 = 90_000;

/// 校验并归一化客户端声明的 LAN 网段。
///
/// 每项必须是合法 IPv4 CIDR；归一化为网络地址形式（如 `192.168.10.5/24` → `192.168.10.0/24`）。
/// 非法项返回 [`AppError::Config`]。
pub(crate) fn normalize_subnets(subnets: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for s in subnets {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        let net: Ipv4Net = s
            .parse()
            .map_err(|_| AppError::Validation(format!("非法的 LAN 网段 CIDR：{s}")))?;
        // 拒绝默认路由（0.0.0.0/0）：用户态/内核客户端按 allowed_route 加 `dev tun` 路由，
        // 缺少 endpoint 旁路时 0.0.0.0/0 会把发往服务端的 UDP 也卷进隧道形成回环，瘫痪连接。
        // 全隧道是独立特性（需 endpoint carve-out），尚未支持，这里直接拒绝以免误配。
        if net.prefix_len() == 0 {
            return Err(AppError::Validation(format!(
                "暂不支持全隧道网段（0.0.0.0/0）：{s}；请改用具体 LAN 网段"
            )));
        }
        let normalized = format!("{}/{}", net.network(), net.prefix_len());
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    Ok(out)
}

/// 逗号分隔的网段串 → 去空项的 Vec（peers.routed_subnets 等列存储的统一解析）。
fn parse_csv_subnets(csv: &str) -> Vec<String> {
    csv.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 宽松归一化 server_routes(env 默认值 / DB 种子两条路径共用)。
///
/// 逐项过 [`normalize_subnets`]:合法项归一化保留,非法项 / `0.0.0.0/0` 跳过并告警——
/// **不**让单个 typo 拖垮启动。关键安全意义:env 旁路若直接把 `0.0.0.0/0` 或任意网段种进
/// server_routes,会被 [`compute_allowed_routes`](PeerService::compute_allowed_routes) 当作
/// 未分组用户的允许集合,从而把所有站点 LAN 泄漏出去(甚至下发全隧道);这里统一兜住。
fn normalize_server_routes_lenient(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        match normalize_subnets(std::slice::from_ref(r)) {
            Ok(v) => {
                for s in v {
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(route = %r, error = %e, "忽略非法 server_route(env/DB 种子)")
            }
        }
    }
    out
}

/// 已渲染的配置文件下载内容。
#[derive(Debug, Clone)]
pub struct PeerConfigDownload {
    pub filename: String,
    pub content: String,
}

#[derive(Clone)]
pub struct PeerService {
    pub peer_repo: SqlitePeerRepository,
    config_repo: SqliteSystemConfigRepository,
    /// 用户组仓库:注册时据此查成员所属组的可路由网段（访问控制）。
    user_group_repo: SqliteUserGroupRepository,
    control: Arc<dyn WireGuardControl>,
    ip_pool: Arc<Mutex<IpPool>>,
    subnet: Ipv4Net,
    server_endpoint: String,
    /// 服务端自身网关的网段（如所在 Docker 网络），下发给**未分组**客户端的默认 allowed_routes。
    /// 运行时可变（admin 后台编辑），变更持久化到 system_config。
    server_routes: Arc<RwLock<Vec<String>>>,
}

impl PeerService {
    /// 构造 PeerService。`ip_pool` 应已由调用方用 peers 表中已占用 IP 回填。
    pub fn new(
        peer_repo: SqlitePeerRepository,
        config_repo: SqliteSystemConfigRepository,
        user_group_repo: SqliteUserGroupRepository,
        control: Arc<dyn WireGuardControl>,
        ip_pool: IpPool,
        server_endpoint: String,
        server_routes: Vec<String>,
    ) -> Self {
        let subnet = ip_pool.subnet();
        Self {
            peer_repo,
            config_repo,
            user_group_repo,
            control,
            ip_pool: Arc::new(Mutex::new(ip_pool)),
            subnet,
            server_endpoint,
            server_routes: Arc::new(RwLock::new(server_routes)),
        }
    }

    /// 当前服务端 LAN 网段（快照）。
    pub async fn server_routes(&self) -> Vec<String> {
        self.server_routes.read().await.clone()
    }

    /// 更新服务端 LAN 网段：校验/规整 CIDR → 持久化 system_config → 更新内存。
    ///
    /// 返回规整后的网段。变更对**新接入/重连**的客户端立即生效（其 `allowed_routes`
    /// 由 [`compute_allowed_routes`](Self::compute_allowed_routes) 实时计算）；
    /// 已连接的客户端需重连后才会拿到新网段。
    pub async fn set_server_routes(&self, subnets: &[String]) -> Result<Vec<String>> {
        let normalized = normalize_subnets(subnets)?;
        self.config_repo
            .set(KEY_SERVER_ROUTES, &normalized.join(","))
            .await?;
        *self.server_routes.write().await = normalized.clone();
        tracing::info!(routes = ?normalized, "服务端 LAN 网段已更新");
        Ok(normalized)
    }

    /// 服务端 WireGuard 公钥（system info 展示）。
    pub fn server_public_key_str(&self) -> &str {
        self.server_public_key()
    }

    /// 服务端 endpoint（system info 展示）。
    pub fn server_endpoint(&self) -> &str {
        &self.server_endpoint
    }

    /// VPN 子网 CIDR（system info 展示）。
    pub fn vpn_subnet_cidr(&self) -> String {
        self.subnet_cidr()
    }

    fn server_public_key(&self) -> &str {
        self.control.server_public_key()
    }

    fn subnet_cidr(&self) -> String {
        self.subnet.to_string()
    }

    /// Story 4.5：注册节点。
    ///
    /// 同一 user 已有非 deleted peer → 复用其 vpn_ip，更新公钥/设备名/os_info；
    /// 否则分配新 IP 并插入。最后调 control.configure_peer。
    pub async fn register(
        &self,
        user_id: &str,
        req: &PeerRegisterRequest,
    ) -> Result<PeerRegisterResponse> {
        // 校验并归一化客户端声明的 LAN 网段（站点网关）。
        let subnets = normalize_subnets(&req.routed_subnets)?;
        let subnets_csv = subnets.join(",");
        // 站点网段不得与其他节点重叠（否则 wg allowed-ips 互抢、站点静默不可达）。
        self.ensure_no_subnet_collision(&subnets, None, Some(user_id))
            .await?;

        let existing = self.peer_repo.find_active_by_user(user_id).await?;
        // 重注册前留存旧公钥/旧网段：换公钥时摘除幽灵 peer、缩减网段时清理残留路由。
        let old_pubkey = existing.as_ref().map(|p| p.wg_public_key.clone());
        let old_routed: Vec<String> = existing
            .as_ref()
            .map(|p| {
                p.routed_subnets
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let vpn_ip: String = match existing {
            Some(peer) => {
                // 复用既有 IP，更新注册信息（公钥冲突会返回 DuplicateResource）。
                self.peer_repo
                    .update_registration(
                        &peer.id,
                        &req.device_name,
                        &req.wg_public_key,
                        req.os_info.as_deref(),
                        &subnets_csv,
                    )
                    .await?;
                peer.vpn_ip
            }
            None => {
                let ip = {
                    let mut pool = self.ip_pool.lock().await;
                    pool.allocate()?
                };
                let ip_str = ip.to_string();
                let id = Uuid::now_v7().to_string();
                match self
                    .peer_repo
                    .insert(
                        &id,
                        user_id,
                        &req.device_name,
                        &req.wg_public_key,
                        &ip_str,
                        req.os_info.as_deref(),
                        &subnets_csv,
                    )
                    .await
                {
                    Ok(_) => ip_str,
                    Err(e) => {
                        // 插入失败（如公钥冲突）→ 归还刚分配的 IP，避免泄漏。
                        self.ip_pool.lock().await.release(ip);
                        return Err(e);
                    }
                }
            }
        };

        let vpn_ip_parsed: Ipv4Addr = vpn_ip
            .parse()
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        self.control
            .configure_peer(&WgPeerConfig {
                public_key: req.wg_public_key.clone(),
                vpn_ip: vpn_ip_parsed,
                endpoint: None,
                allowed_subnets: subnets.clone(),
            })
            .await?;

        // 换公钥：摘除旧 wg peer，避免接口上残留持旧公钥（及其旧 allowed-ips）的幽灵 peer。
        if let Some(old) = &old_pubkey {
            if old != &req.wg_public_key {
                let _ = self.control.remove_peer(old).await;
            }
        }
        // 缩减网段：删除不再声明的站点网段的残留 OS 路由（best-effort，避免黑洞）。
        // 但保留仍被其他活跃网关接管的网段，避免删掉别人正在用的路由。
        let removed: Vec<String> = old_routed
            .into_iter()
            .filter(|s| !subnets.contains(s))
            .collect();
        let removed = self
            .filter_releasable_routes(removed, None, Some(user_id))
            .await?;
        if !removed.is_empty() {
            let _ = self.control.remove_routes(&removed).await;
        }

        Ok(PeerRegisterResponse {
            vpn_ip,
            server_public_key: self.server_public_key().to_string(),
            server_endpoint: self.server_endpoint.clone(),
            vpn_subnet: self.subnet_cidr(),
            // 客户端应路由：VPN 子网 + 组网段(或全局默认) + 其他站点的 LAN 网段（不含自己声明的）。
            allowed_routes: self.compute_allowed_routes(user_id, &subnets).await?,
        })
    }

    /// 计算客户端应导入隧道的网段（访问控制）：
    /// - 始终含 VPN 子网；
    /// - 用户**已分组** → 用本组配置的可路由网段（多组取并集；空集=仅放行 VPN 子网）；
    /// - 用户**未分组** → 回退全局 `server_routes`；
    /// - 站点网关 peer 自报的 LAN 网段**仅当落入上述允许集合内**（被某条允许网段覆盖或
    ///   等于）才叠加，受组/服务端路由约束——避免任意站点 LAN 无差别下发给所有客户端。
    ///   排除本机自报的、去重。
    async fn compute_allowed_routes(
        &self,
        user_id: &str,
        own_subnets: &[String],
    ) -> Result<Vec<String>> {
        let mut routes = vec![self.subnet_cidr()];
        // 组路由优先（访问控制，多组取并集）；无任何组回退全局 server_routes。
        let base: Vec<String> = match self.user_group_repo.routes_for_user(user_id).await? {
            Some(group_routes) => group_routes,
            None => self.server_routes.read().await.clone(),
        };
        // base 解析为网段集合，用于判定站点网关 LAN 是否落在该用户的允许范围内。
        let allow_nets: Vec<Ipv4Net> = base.iter().filter_map(|s| s.parse().ok()).collect();
        // 本网关自报 LAN 的网段形式,用于按 CIDR **包含**(而非精确字符串相等)做自排除:
        // 若组/服务端路由是本网关 LAN 的子集(如本地 /24 下发组路由 /25),精确相等判不出来,
        // 会把更具体的 /25 塞回该网关自己的隧道,使它对自己半个 LAN 的本地流量被卷进隧道兜圈。
        let own_nets: Vec<Ipv4Net> = own_subnets.iter().filter_map(|s| s.parse().ok()).collect();
        for s in base {
            if routes.contains(&s) {
                continue;
            }
            let covered_by_own = match s.parse::<Ipv4Net>() {
                Ok(net) => own_nets.iter().any(|o| o.contains(&net)),
                Err(_) => own_subnets.contains(&s),
            };
            if !covered_by_own {
                routes.push(s);
            }
        }
        // 站点网关 LAN：仅当被允许集合覆盖时才下发（关闭“任意站点 LAN 泄漏给所有人”）。
        // 仅扫描声明了网段的网关 peer（list_active_gateway_routes），不再全表扫描所有活跃 peer。
        for (_pid, _uid, csv) in self.peer_repo.list_active_gateway_routes().await? {
            for s in csv.split(',').filter(|s| !s.is_empty()) {
                if own_subnets.iter().any(|o| o == s) || routes.iter().any(|r| r == s) {
                    continue;
                }
                if let Ok(net) = s.parse::<Ipv4Net>() {
                    if allow_nets.iter().any(|a| a.contains(&net)) {
                        routes.push(s.to_string());
                    }
                }
            }
        }
        Ok(routes)
    }

    /// 两个 IPv4 网段是否重叠（CIDR 性质：要么不相交，要么一方包含另一方）。
    fn nets_overlap(a: &Ipv4Net, b: &Ipv4Net) -> bool {
        a.contains(b) || b.contains(a)
    }

    /// 从待删除的残留 OS 路由中剔除仍被**其他活跃网关 peer** 声明的网段。
    ///
    /// 防黑洞：force_removed 的网关让出某网段后可能被另一节点接管；当原网关重注册并缩减
    /// 网段时，若直接 `ip route del` 这些网段会删掉接管者正在用的路由。故按精确 CIDR 串
    /// 比对，凡仍被其他活跃网关声明者一律保留（不删）。`exclude_*`：排除“自己”。
    async fn filter_releasable_routes(
        &self,
        removed: Vec<String>,
        exclude_peer_id: Option<&str>,
        exclude_user_id: Option<&str>,
    ) -> Result<Vec<String>> {
        if removed.is_empty() {
            return Ok(removed);
        }
        let mut still_claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (pid, uid, csv) in self.peer_repo.list_active_gateway_routes().await? {
            if exclude_peer_id == Some(pid.as_str()) || exclude_user_id == Some(uid.as_str()) {
                continue;
            }
            for s in csv.split(',').filter(|s| !s.is_empty()) {
                still_claimed.insert(s.to_string());
            }
        }
        Ok(removed
            .into_iter()
            .filter(|s| !still_claimed.contains(s))
            .collect())
    }

    /// 校验拟声明的站点网段不与**其他**活跃网关 peer 已声明的网段重叠。
    ///
    /// WireGuard 同一接口的 allowed-ips 必须各 peer 互不重叠：若两个网关都声明同一/重叠
    /// CIDR，`wg set` 会把该前缀从先前 peer 抢到后者，导致前一站点 LAN 静默不可达。
    /// 此处在注册 / 改路由的写入前主动拒绝重叠，避免无声黑洞。
    /// `exclude_peer_id` / `exclude_user_id`：排除“自己”（改路由按 peer、重注册按 user）。
    async fn ensure_no_subnet_collision(
        &self,
        new_subnets: &[String],
        exclude_peer_id: Option<&str>,
        exclude_user_id: Option<&str>,
    ) -> Result<()> {
        if new_subnets.is_empty() {
            return Ok(());
        }
        let news: Vec<Ipv4Net> = new_subnets.iter().filter_map(|s| s.parse().ok()).collect();
        // 站点网段不得覆盖/重叠 VPN 子网本身。否则 configure_peer 会对该网段执行
        // `ip route replace <subnet> dev wg0`,把服务端到 VPN 子网(乃至其超网,如 10.0.0.0/8)
        // 的路由整段劫持进 wg 接口,断掉服务端对未分配 VPN IP / 同段其它主机的可达性。
        for n in &news {
            if Self::nets_overlap(n, &self.subnet) {
                return Err(AppError::Validation(format!(
                    "站点网段 {n} 与 VPN 子网 {} 重叠,请改用不冲突的网段",
                    self.subnet
                )));
            }
        }
        for (pid, uid, csv) in self.peer_repo.list_active_gateway_routes().await? {
            if exclude_peer_id == Some(pid.as_str()) || exclude_user_id == Some(uid.as_str()) {
                continue;
            }
            for s in csv.split(',').filter(|s| !s.is_empty()) {
                if let Ok(other) = s.parse::<Ipv4Net>() {
                    for n in &news {
                        if Self::nets_overlap(n, &other) {
                            return Err(AppError::Validation(format!(
                                "站点网段 {n} 与另一节点已声明的 {other} 冲突，请避免重叠"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Story 4.6：心跳。无活跃 peer → PeerNotFound。
    ///
    /// 返回该节点**当前**应导入隧道的网段(按最新组/服务端/站点配置实时计算),供客户端
    /// 增量更新本地路由表——使组/网段变更无需重连即生效(P1.4)。
    pub async fn heartbeat(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
    ) -> Result<Vec<String>> {
        // 本节点自报的站点 LAN(后续从 allowed_routes 中排除自己声明的)。
        let own = self
            .peer_repo
            .find_active_by_user(user_id)
            .await?
            .map(|p| parse_csv_subnets(&p.routed_subnets))
            .unwrap_or_default();
        self.heartbeat_with_own(user_id, endpoint, now_ms, &own).await
    }

    /// [`heartbeat`] / [`heartbeat_checked`] 的共用内核:打卡 + 据已知自报网段算 allowed_routes。
    /// 调用方传入已查到的 `own`(自报站点 LAN),避免再查一次 peer 行(心跳是每 30s/每在线
    /// 客户端的热路径)。无活跃 peer → 打卡 affected==0 → PeerNotFound。
    async fn heartbeat_with_own(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
        own: &[String],
    ) -> Result<Vec<String>> {
        let affected = self
            .peer_repo
            .touch_heartbeat(user_id, endpoint, now_ms)
            .await?;
        if affected == 0 {
            return Err(AppError::PeerNotFound);
        }
        self.compute_allowed_routes(user_id, own).await
    }

    /// Story 4.7：注销当前 user 的 peer。无活跃 peer → PeerNotFound。
    pub async fn delete_me(&self, user_id: &str) -> Result<()> {
        let peer = self
            .peer_repo
            .find_active_by_user(user_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        self.control.remove_peer(&peer.wg_public_key).await?;
        self.peer_repo.mark_deleted_by_user(user_id).await?;
        Ok(())
    }

    /// Story 4.7：渲染当前 user peer 的客户端配置文件。无活跃 peer → PeerNotFound。
    pub async fn render_config(&self, user_id: &str) -> Result<PeerConfigDownload> {
        let peer = self
            .peer_repo
            .find_active_by_user(user_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        let client_ip: Ipv4Addr = peer
            .vpn_ip
            .parse()
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        let dns = self
            .subnet
            .hosts()
            .next()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "10.8.0.1".to_string());
        // 分隧道：路由 VPN 子网 + 其他站点 LAN 网段（排除本机自报的）。
        let own = peer
            .routed_subnets
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let allowed_ips = self.compute_allowed_routes(user_id, &own).await?;
        let content = render_client_config(
            CLIENT_PRIVATE_KEY_PLACEHOLDER,
            client_ip,
            self.subnet.prefix_len(),
            &dns,
            self.server_public_key(),
            &self.server_endpoint,
            PERSISTENT_KEEPALIVE,
            &allowed_ips,
        );
        Ok(PeerConfigDownload {
            filename: "vpn.conf".to_string(),
            content,
        })
    }

    /// 离线检测：把心跳超过阈值的 online peer 标记为 offline。返回标记行数。
    pub async fn scan_offline(&self, now_ms: i64) -> Result<u64> {
        let cutoff = now_ms - OFFLINE_THRESHOLD_MS;
        self.peer_repo.mark_stale_offline(cutoff).await
    }

    /// Story 5.5：心跳（admin 视角）。同 `heartbeat`，但若 peer 已被强制下线则拒绝。
    ///
    /// 强制下线后客户端下次心跳应失败，提示重新登录。
    pub async fn heartbeat_checked(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
    ) -> Result<Vec<String>> {
        // touch_heartbeat 的 SQL 排除 status='deleted'，但 force_removed 仍会被更新；
        // 故先显式检查活跃 peer 状态。这次查到的 peer 直接复用,避免 heartbeat 内再查一次。
        let peer = self.peer_repo.find_active_by_user(user_id).await?;
        if let Some(p) = &peer {
            if p.status == "force_removed" {
                // 复用 TokenExpired（401）→ 客户端据此提示重新登录。
                return Err(AppError::TokenExpired);
            }
        }
        let own = peer
            .map(|p| parse_csv_subnets(&p.routed_subnets))
            .unwrap_or_default();
        self.heartbeat_with_own(user_id, endpoint, now_ms, &own).await
    }

    /// Story 5.5：admin 强制下线指定 peer。
    ///
    /// 从 WireGuard runtime 移除 + 把 peers.status 置为 'force_removed'。
    /// peer 不存在 → PeerNotFound。
    pub async fn force_remove(&self, peer_id: &str) -> Result<()> {
        let peer = self
            .peer_repo
            .find_by_id(peer_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        self.control.remove_peer(&peer.wg_public_key).await?;
        self.peer_repo.mark_force_removed(peer_id).await?;
        Ok(())
    }

    /// admin 彻底删除指定 peer：摘除 WireGuard peer + 物理删库行 + **回收 VPN IP**。
    ///
    /// 与 [`force_remove`](Self::force_remove)（软删除，保留记录与 IP 占用）不同，
    /// 本方法让节点从列表彻底消失，且释放其 VPN IP 供后续节点复用。peer 不存在 →
    /// PeerNotFound。WireGuard 摘除为 best-effort（已 force_removed 的 peer 可能不在
    /// runtime，不应因此阻塞删除）。
    pub async fn purge(&self, peer_id: &str) -> Result<()> {
        let peer = self
            .peer_repo
            .find_by_id(peer_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        // 已下线的 peer 可能已不在 runtime；忽略摘除错误以保证删除可完成。
        let _ = self.control.remove_peer(&peer.wg_public_key).await;
        self.peer_repo.delete_by_id(peer_id).await?;
        // 回收内存 IP 池中的占用（启动回填与 release 共同维护一致性）。
        if let Ok(ip) = peer.vpn_ip.parse::<Ipv4Addr>() {
            self.ip_pool.lock().await.release(ip);
        }
        Ok(())
    }

    /// 用户被**删除**时联动清理其节点:摘除 WireGuard peer + 物理删库 + 回收 VPN IP。
    /// 无活跃 peer 则静默成功(用户可能从未接入)。
    pub async fn purge_by_user(&self, user_id: &str) -> Result<()> {
        // 摘除活跃 peer 的 WireGuard runtime（best-effort，可能已不在 runtime）。
        if let Some(peer) = self.peer_repo.find_active_by_user(user_id).await? {
            let _ = self.control.remove_peer(&peer.wg_public_key).await;
        }
        // 硬删该用户**全部** peer 行（含历史 'deleted' 软删行）并回收各自 IP。必须先于删除
        // users 行：peers.user_id -> users.id 外键无级联，残留任一 peer 行都会让删用户报外键冲突。
        let freed = self.peer_repo.delete_all_by_user(user_id).await?;
        if !freed.is_empty() {
            let mut pool = self.ip_pool.lock().await;
            for ip in &freed {
                if let Ok(p) = ip.parse::<Ipv4Addr>() {
                    pool.release(p);
                }
            }
            tracing::info!(user_id, count = freed.len(), "用户删除:已清除其全部节点并回收 IP");
        }
        Ok(())
    }

    /// 用户被**禁用**时联动踢隧道:摘除 WireGuard peer + 标记 force_removed(保留记录与 IP)。
    /// 重新启用并重连后 force_removed 会被清除而恢复。无活跃 peer 则静默成功。
    pub async fn force_remove_by_user(&self, user_id: &str) -> Result<()> {
        if let Some(peer) = self.peer_repo.find_active_by_user(user_id).await? {
            // best-effort 摘除：用户已在 update_status 中被禁用并吊销会话，WireGuard 摘除
            // 失败（接口已 down/权限/runtime 无此 peer）不应让禁用接口整体返回 500、
            // 造成“库里已禁用、API 却报错”的不一致。与 purge_by_user 保持一致。
            if let Err(e) = self.control.remove_peer(&peer.wg_public_key).await {
                tracing::warn!(user_id, error = %e, "用户禁用:摘除 WireGuard peer 失败（忽略，仍标记下线）");
            }
            self.peer_repo.mark_force_removed(&peer.id).await?;
            tracing::info!(user_id, peer_id = %peer.id, "用户禁用:已强制下线其节点");
        }
        Ok(())
    }

    /// admin 编辑指定 peer 的路由网段（异地组网）。
    ///
    /// 校验/归一化 CIDR → 持久化 → 重新下发 WireGuard 配置（更新 allowed-ips 与路由）。
    /// peer 不存在 → PeerNotFound；非法 CIDR → Config。
    pub async fn update_peer_routes(&self, peer_id: &str, subnets: &[String]) -> Result<()> {
        let peer = self
            .peer_repo
            .find_by_id(peer_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        let normalized = normalize_subnets(subnets)?;
        // 不得与其他节点的站点网段重叠（否则 wg allowed-ips 互抢、站点静默不可达）。
        self.ensure_no_subnet_collision(&normalized, Some(peer_id), None)
            .await?;
        let old_routed: Vec<String> = peer
            .routed_subnets
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        self.peer_repo
            .update_routed_subnets(peer_id, &normalized.join(","))
            .await?;
        // 重新下发到 WireGuard（仅对未删除/未强制下线的 peer 生效）。
        if peer.status != "deleted" && peer.status != "force_removed" {
            if let Ok(ip) = peer.vpn_ip.parse::<Ipv4Addr>() {
                self.control
                    .configure_peer(&WgPeerConfig {
                        public_key: peer.wg_public_key,
                        vpn_ip: ip,
                        endpoint: None,
                        allowed_subnets: normalized.clone(),
                    })
                    .await?;
                // 缩减网段：删除被移除网段的残留 OS 路由（best-effort，避免黑洞）。
                // 保留仍被其他活跃网关接管的网段，避免删掉别人正在用的路由。
                let removed: Vec<String> = old_routed
                    .into_iter()
                    .filter(|s| !normalized.contains(s))
                    .collect();
                let removed = self
                    .filter_releasable_routes(removed, Some(peer_id), None)
                    .await?;
                if !removed.is_empty() {
                    let _ = self.control.remove_routes(&removed).await;
                }
            }
        }
        Ok(())
    }

    /// Story 5.5：admin peer 列表（JOIN users）。
    pub async fn list_admin_peers(
        &self,
        query: &vpn_api_types::peer::AdminPeerQuery,
    ) -> Result<vpn_api_types::Page<vpn_api_types::peer::AdminPeerView>> {
        use crate::repositories::peer_repo_sqlite::AdminPeerFilter;
        let page = query.page.unwrap_or(1).max(1);
        let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
        let filter = AdminPeerFilter {
            search: query.search.clone(),
            status: query.status.clone(),
            page,
            page_size,
        };
        let total = self.peer_repo.count_admin(&filter).await? as u64;
        let rows = self.peer_repo.list_admin(&filter).await?;
        let items = rows
            .into_iter()
            .map(|r| vpn_api_types::peer::AdminPeerView {
                id: r.id,
                user_id: r.user_id,
                username: r.username,
                email: r.email,
                device_name: r.device_name,
                wg_public_key: r.wg_public_key,
                vpn_ip: r.vpn_ip,
                endpoint: r.endpoint,
                os_info: r.os_info,
                last_seen_at: r.last_seen_at,
                status: r.status,
                created_at: r.created_at,
                routed_subnets: r
                    .routed_subnets
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            })
            .collect();
        Ok(vpn_api_types::Page::new(items, total, page, page_size))
    }

    /// 服务端公钥（供 system_info 等只读展示用）。
    pub fn server_public_key_string(&self) -> String {
        self.server_public_key().to_string()
    }
}

/// 装配 PeerService（默认 Noop 后端，用于测试 / 无特权环境）。
pub async fn build_peer_service(
    peer_repo: SqlitePeerRepository,
    config_repo: &SqliteSystemConfigRepository,
    subnet: Ipv4Net,
    server_endpoint: String,
) -> Result<PeerService> {
    build_peer_service_with_backend(
        peer_repo,
        config_repo,
        subnet,
        server_endpoint,
        "noop",
        "wg0",
        51820,
        Vec::new(),
    )
    .await
}

/// 装配 PeerService，可选 WireGuard 后端。
///
/// `backend`：`"kernel"` 使用 Linux 内核 WireGuard（需内核 WG 模块 + root/CAP_NET_ADMIN + `wg`）；
/// `"userspace"` 使用用户态 `wireguard-go`（仅需 `/dev/net/tun`，适配无内核 WG 的老内核）；
/// 其余值（含 `"noop"`）使用无副作用的记账实现。
#[allow(clippy::too_many_arguments)]
pub async fn build_peer_service_with_backend(
    peer_repo: SqlitePeerRepository,
    config_repo: &SqliteSystemConfigRepository,
    subnet: Ipv4Net,
    server_endpoint: String,
    backend: &str,
    iface: &str,
    listen_port: u16,
    server_routes: Vec<String>,
) -> Result<PeerService> {
    // 1. load-or-generate 服务端 WG 密钥（私钥 + 公钥都需要）
    let (private_key, public_key) = match config_repo.get(KEY_SERVER_WG_PRIVATE).await? {
        Some(private) => {
            let public = public_key_from_private(&private)?;
            (private, public)
        }
        None => {
            let kp = generate_keypair();
            config_repo
                .set(KEY_SERVER_WG_PRIVATE, &kp.private_key)
                .await?;
            config_repo
                .set(KEY_SERVER_WG_PUBLIC, &kp.public_key)
                .await?;
            (kp.private_key, kp.public_key)
        }
    };

    // 2. 构造 IpPool 并用现存 peers 回填
    let mut ip_pool = IpPool::new(subnet);
    for ip_str in peer_repo.list_active_vpn_ips().await? {
        match ip_str.parse::<Ipv4Addr>() {
            Ok(ip) => ip_pool.mark_used(ip),
            Err(e) => {
                tracing::warn!(vpn_ip = %ip_str, error = %e, "peers 表中存在非法 vpn_ip，回填跳过")
            }
        }
    }

    // 3. 选择 WireGuard 后端 → 解析为按序尝试的 mode 列表：
    //    - "kernel"：仅内核 WireGuard（需内核模块）
    //    - "userspace"：仅用户态 wireguard-go（无内核模块亦可，适配老内核）
    //    - "auto"：先内核、失败回退用户态（现代机器走快的内核态，老内核自动降级）
    //    - 其余（含 "noop"）：无副作用记账实现
    let modes: &[WgMode] = match backend {
        "kernel" => &[WgMode::Kernel],
        "userspace" => &[WgMode::Userspace],
        "auto" => &[WgMode::Kernel, WgMode::Userspace],
        _ => &[],
    };
    let control: Arc<dyn WireGuardControl> = if !modes.is_empty() {
        let server_addr = ip_pool
            .server_addr()
            .ok_or_else(|| AppError::WireGuard("子网无可用服务端地址".to_string()))?;
        // 依次尝试各 mode，第一个成功即采用；最后一个仍失败则向上报错。
        let mut started: Option<(KernelWireGuardControl, WgMode)> = None;
        for (idx, &mode) in modes.iter().enumerate() {
            match KernelWireGuardControl::start(
                iface,
                &private_key,
                &public_key,
                server_addr,
                subnet.prefix_len(),
                listen_port,
                mode,
            )
            .await
            {
                Ok(kc) => {
                    started = Some((kc, mode));
                    break;
                }
                Err(e) if idx + 1 < modes.len() => {
                    tracing::warn!(?mode, error = %e, "WireGuard 后端启动失败，回退下一后端");
                }
                Err(e) => return Err(e),
            }
        }
        let (kc, mode) = started.expect("modes 非空且失败已处理");
        // 启动恢复：把已存在的 active peers 重新下发到接口
        for (pubkey, ip_str, subnets_csv) in peer_repo.list_active_peer_keys().await? {
            if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                let cfg = WgPeerConfig {
                    public_key: pubkey,
                    vpn_ip: ip,
                    endpoint: None,
                    allowed_subnets: subnets_csv
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect(),
                };
                if let Err(e) = kc.configure_peer(&cfg).await {
                    tracing::warn!(error = %e, "启动恢复 peer 配置失败");
                }
            }
        }
        tracing::info!(iface, ?mode, "使用 WireGuard 后端（真实隧道）");
        Arc::new(kc)
    } else {
        tracing::info!("使用 Noop WireGuard 后端（无真实隧道）");
        Arc::new(NoopWireGuardControl::new(public_key))
    };

    // 服务端 LAN 网段：DB 中持久化的值优先（admin 可运行时改）；缺省时用启动传入的
    // `VPN_SERVER_ROUTES` 默认值并落库一次作为初始种子。
    let effective_routes = match config_repo.get(KEY_SERVER_ROUTES).await? {
        // DB 中的值理应已由 set_server_routes 归一化,这里再过一遍兜底(幂等)。
        Some(csv) => {
            let raw: Vec<String> = csv
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            normalize_server_routes_lenient(&raw)
        }
        // 首次启动:用 VPN_SERVER_ROUTES 默认值,**归一化后**落库一次作为种子
        // (拒绝 0.0.0.0/0 与非法 CIDR,避免 env 旁路绕过校验)。
        None => {
            let normalized = normalize_server_routes_lenient(&server_routes);
            if !normalized.is_empty() {
                config_repo
                    .set(KEY_SERVER_ROUTES, &normalized.join(","))
                    .await?;
            }
            normalized
        }
    };

    let user_group_repo = SqliteUserGroupRepository::new(config_repo.pool().clone());

    Ok(PeerService::new(
        peer_repo,
        config_repo.clone(),
        user_group_repo,
        control,
        ip_pool,
        server_endpoint,
        effective_routes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use vpn_wireguard::NoopWireGuardControl;

    async fn setup_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:peer_service_test_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-1', 'alice', 'a@e.com', 'h', 'user', 'active', 0, 0, 0),
                      ('user-2', 'bob', 'b@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn service(pool: SqlitePool) -> PeerService {
        let pool_net: Ipv4Net = "10.8.0.0/24".parse().unwrap();
        let ip_pool = IpPool::new(pool_net);
        let control = Arc::new(NoopWireGuardControl::new("SERVER_PUB"));
        PeerService::new(
            SqlitePeerRepository::new(pool.clone()),
            SqliteSystemConfigRepository::new(pool.clone()),
            SqliteUserGroupRepository::new(pool),
            control,
            ip_pool,
            "vpn.example.com:51820".to_string(),
            Vec::new(),
        )
    }

    fn service_with_server_routes(pool: SqlitePool, routes: Vec<String>) -> PeerService {
        let ip_pool = IpPool::new("10.8.0.0/24".parse().unwrap());
        PeerService::new(
            SqlitePeerRepository::new(pool.clone()),
            SqliteSystemConfigRepository::new(pool.clone()),
            SqliteUserGroupRepository::new(pool),
            Arc::new(NoopWireGuardControl::new("SERVER_PUB")),
            ip_pool,
            "vpn.example.com:51820".to_string(),
            routes,
        )
    }

    #[tokio::test]
    async fn server_routes_propagate_to_client_allowed_routes() {
        let svc =
            service_with_server_routes(setup_pool().await, vec!["172.31.100.0/24".to_string()]);
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        assert!(resp.allowed_routes.contains(&"10.8.0.0/24".to_string()));
        assert!(resp.allowed_routes.contains(&"172.31.100.0/24".to_string()));
    }

    #[tokio::test]
    async fn group_routes_override_global_for_member() {
        let pool = setup_pool().await;
        // 建组 g1 = 192.168.50.0/24,把 user-1 放进组;user-2 不分组。
        let groups = SqliteUserGroupRepository::new(pool.clone());
        groups.insert("g1", "ops", "192.168.50.0/24").await.unwrap();
        groups
            .set_groups("user-1", &["g1".to_string()])
            .await
            .unwrap();

        // 全局默认 = 10.99.0.0/24。
        let svc = service_with_server_routes(pool, vec!["10.99.0.0/24".to_string()]);

        // 组成员:拿到组网段,**不含**全局默认;VPN 子网恒含。
        let r1 = svc.register("user-1", &reg("PKA")).await.unwrap();
        assert!(r1.allowed_routes.contains(&"10.8.0.0/24".to_string()));
        assert!(r1.allowed_routes.contains(&"192.168.50.0/24".to_string()));
        assert!(!r1.allowed_routes.contains(&"10.99.0.0/24".to_string()));

        // 未分组成员:回退全局默认,不含组网段。
        let r2 = svc.register("user-2", &reg("PKB")).await.unwrap();
        assert!(r2.allowed_routes.contains(&"10.99.0.0/24".to_string()));
        assert!(!r2.allowed_routes.contains(&"192.168.50.0/24".to_string()));
    }

    #[tokio::test]
    async fn purge_by_user_removes_peer_and_releases_ip() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PKX")).await.unwrap();
        assert!(svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .is_some());
        svc.purge_by_user("user-1").await.unwrap();
        assert!(svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .is_none());
        // IP 已回收:再次注册仍成功。
        let r = svc.register("user-1", &reg("PKY")).await.unwrap();
        assert!(!r.vpn_ip.is_empty());
    }

    #[tokio::test]
    async fn purge_by_user_without_peer_is_noop() {
        let svc = service(setup_pool().await);
        svc.purge_by_user("user-2").await.unwrap();
    }

    #[tokio::test]
    async fn force_remove_by_user_marks_force_removed() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PKZ")).await.unwrap();
        svc.force_remove_by_user("user-1").await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(peer.status, "force_removed");
    }

    #[tokio::test]
    async fn set_server_routes_normalizes_persists_and_affects_new_registrations() {
        let svc = service(setup_pool().await);
        // 初始无服务端网段
        assert!(svc.server_routes().await.is_empty());
        // 设置（含需规整的主机位 + 去重）
        let saved = svc
            .set_server_routes(&[
                "192.168.50.10/24".to_string(),
                "192.168.50.0/24".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(saved, vec!["192.168.50.0/24".to_string()]);
        assert_eq!(
            svc.server_routes().await,
            vec!["192.168.50.0/24".to_string()]
        );
        // 新注册的客户端 allowed_routes 立即含新网段
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        assert!(resp.allowed_routes.contains(&"192.168.50.0/24".to_string()));
    }

    #[tokio::test]
    async fn set_server_routes_rejects_invalid_cidr() {
        let svc = service(setup_pool().await);
        let err = svc
            .set_server_routes(&["not-a-cidr".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    fn reg(pk: &str) -> PeerRegisterRequest {
        PeerRegisterRequest {
            wg_public_key: pk.to_string(),
            device_name: "MBP".to_string(),
            os_info: Some("macOS".to_string()),
            routed_subnets: Vec::new(),
        }
    }

    fn reg_with_subnets(pk: &str, subnets: &[&str]) -> PeerRegisterRequest {
        PeerRegisterRequest {
            wg_public_key: pk.to_string(),
            device_name: "GW".to_string(),
            os_info: None,
            routed_subnets: subnets.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn register_rejects_invalid_subnet() {
        let svc = service(setup_pool().await);
        let err = svc
            .register("user-1", &reg_with_subnets("PK1", &["not-a-cidr"]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn site_to_site_routes_are_access_controlled() {
        let svc = service(setup_pool().await);
        // 站点网关 B 声明 LAN 192.168.20.0/24（会被归一化）。
        svc.register("user-2", &reg_with_subnets("PKB", &["192.168.20.5/24"]))
            .await
            .unwrap();
        // 访问控制：未在 server_routes / 组路由中放行该 LAN 的客户端 A，只拿到 VPN 子网，
        // 拿不到 B 的站点 LAN（站点 LAN 不再无差别下发给所有客户端）。
        let resp_a = svc.register("user-1", &reg("PKA")).await.unwrap();
        assert!(resp_a.allowed_routes.contains(&"10.8.0.0/24".to_string()));
        assert!(!resp_a
            .allowed_routes
            .contains(&"192.168.20.0/24".to_string()));
        // 把该 LAN 配进全局 server_routes（A 未分组 → 回退 server_routes）后，A 重连即可拿到。
        svc.set_server_routes(&["192.168.20.0/24".to_string()])
            .await
            .unwrap();
        let resp_a2 = svc.register("user-1", &reg("PKA")).await.unwrap();
        assert!(resp_a2
            .allowed_routes
            .contains(&"192.168.20.0/24".to_string()));
        // B 的下载配置不应把自己的 LAN 再路由进隧道（排除自报）。
        let conf_b = svc.render_config("user-2").await.unwrap();
        assert!(conf_b.content.contains("10.8.0.0/24"));
        assert!(!conf_b.content.contains("192.168.20.0/24"));
    }

    #[tokio::test]
    async fn register_rejects_overlapping_site_subnet() {
        let svc = service(setup_pool().await);
        // 网关 B 声明 192.168.20.0/24。
        svc.register("user-2", &reg_with_subnets("PKB", &["192.168.20.0/24"]))
            .await
            .unwrap();
        // 另一节点声明重叠网段（更大范围覆盖之）→ 拒绝，避免 wg allowed-ips 互抢致静默黑洞。
        let err = svc
            .register("user-3", &reg_with_subnets("PKC", &["192.168.0.0/16"]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn register_rejects_default_route_subnet() {
        let svc = service(setup_pool().await);
        // 0.0.0.0/0（全隧道）暂不支持，路由校验处直接拒绝，避免用户态客户端回环瘫痪。
        let err = svc
            .register("user-1", &reg_with_subnets("PK1", &["0.0.0.0/0"]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn register_rejects_subnet_overlapping_vpn_subnet() {
        let svc = service(setup_pool().await); // VPN 子网 10.8.0.0/24
        // 站点网段覆盖 VPN 子网超网（10/8）→ 拒绝：否则服务端 `ip route replace 10.0.0.0/8 dev wg0`
        // 会劫持服务端整段 10/8 路由。
        let err = svc
            .register("user-1", &reg_with_subnets("PK1", &["10.0.0.0/8"]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
        // 精确等于 VPN 子网亦拒绝。
        let err = svc
            .register("user-2", &reg_with_subnets("PK2", &["10.8.0.0/24"]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn purge_by_user_clears_all_rows_so_user_delete_is_fk_safe() {
        let pool = setup_pool().await; // 已 seed users user-1 / user-2（FK 默认开启）
        let svc = service(pool.clone());
        // 注册 → 软删 → 再注册：user-1 名下出现一条 'deleted' 行 + 一条活跃行。
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.peer_repo.mark_deleted_by_user("user-1").await.unwrap();
        svc.register("user-1", &reg("PK2")).await.unwrap();
        let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM peers WHERE user_id = 'user-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before.0, 2);
        // 残留 peer 行时直接删 users 触发外键冲突（peers.user_id -> users.id 无级联）。
        assert!(sqlx::query("DELETE FROM users WHERE id = 'user-1'")
            .execute(&pool)
            .await
            .is_err());
        // purge_by_user 硬删其**全部** peer 行（任意状态）。
        svc.purge_by_user("user-1").await.unwrap();
        let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM peers WHERE user_id = 'user-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after.0, 0);
        // 现在删除 users 行不再外键冲突。
        sqlx::query("DELETE FROM users WHERE id = 'user-1'")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn register_allocates_new_ip() {
        let svc = service(setup_pool().await);
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        assert_eq!(resp.vpn_ip, "10.8.0.2");
        assert_eq!(resp.server_public_key, "SERVER_PUB");
        assert_eq!(resp.server_endpoint, "vpn.example.com:51820");
        assert_eq!(resp.vpn_subnet, "10.8.0.0/24");
        assert_eq!(svc.control.list_peers().await.unwrap(), vec!["PK1"]);
    }

    #[tokio::test]
    async fn register_reuses_ip_for_same_user() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        // 重新注册（换公钥）→ 同 IP
        let r2 = svc.register("user-1", &reg("PK2")).await.unwrap();
        assert_eq!(r1.vpn_ip, r2.vpn_ip);
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.wg_public_key, "PK2");
    }

    #[tokio::test]
    async fn register_different_users_get_different_ips() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        let r2 = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_ne!(r1.vpn_ip, r2.vpn_ip);
    }

    #[tokio::test]
    async fn register_duplicate_public_key_releases_ip() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        // user-2 用 user-1 已占公钥 → DuplicateResource，且 IP 应被归还
        let err = svc.register("user-2", &reg("PK1")).await.unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
        // 归还后，user-2 用新公钥应能拿到本应分配的那个 IP（10.8.0.3）
        let r = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_eq!(r.vpn_ip, "10.8.0.3");
    }

    #[tokio::test]
    async fn heartbeat_unknown_user_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.heartbeat("user-1", None, 1000).await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn heartbeat_sets_online() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.heartbeat("user-1", Some("1.2.3.4:99"), 1000)
            .await
            .unwrap();
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn render_config_contains_interface_and_peer_ip() {
        let svc = service(setup_pool().await);
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        let dl = svc.render_config("user-1").await.unwrap();
        assert_eq!(dl.filename, "vpn.conf");
        assert!(dl.content.contains("[Interface]"));
        assert!(dl
            .content
            .contains(&format!("Address = {}/24", resp.vpn_ip)));
        assert!(dl.content.contains("PublicKey = SERVER_PUB"));
        assert!(dl.content.contains("Endpoint = vpn.example.com:51820"));
        assert!(dl.content.contains(CLIENT_PRIVATE_KEY_PLACEHOLDER));
    }

    #[tokio::test]
    async fn render_config_unknown_user_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.render_config("user-1").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn delete_me_removes_peer_from_control_and_active() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.delete_me("user-1").await.unwrap();
        assert!(svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .is_none());
        assert!(svc.control.list_peers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_me_unknown_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.delete_me("user-1").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn register_after_delete_keeps_same_ip() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.delete_me("user-1").await.unwrap();
        // 注销后重新注册：当前实现不立即释放 IP，但 find_active 排除 deleted，
        // 故会走 allocate 分支拿到下一个 IP（旧 IP 仍在 pool 中被占用）。
        let r2 = svc.register("user-1", &reg("PK2")).await.unwrap();
        assert_ne!(r1.vpn_ip, r2.vpn_ip);
    }

    #[tokio::test]
    async fn build_peer_service_generates_and_persists_key() {
        let pool = setup_pool().await;
        let config_repo = SqliteSystemConfigRepository::new(pool.clone());
        let peer_repo = SqlitePeerRepository::new(pool.clone());
        let subnet: Ipv4Net = "10.8.0.0/24".parse().unwrap();

        let svc = build_peer_service(
            peer_repo.clone(),
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        let pub1 = svc.server_public_key_string();
        assert_eq!(pub1.len(), 44);
        // 公钥应等于从存储私钥推导的值
        let private = config_repo
            .get(KEY_SERVER_WG_PRIVATE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(public_key_from_private(&private).unwrap(), pub1);

        // 再次构造：密钥应稳定（load 而非 regenerate）
        let svc2 = build_peer_service(
            peer_repo,
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(svc2.server_public_key_string(), pub1);
    }

    #[tokio::test]
    async fn build_peer_service_backfills_ip_pool() {
        let pool = setup_pool().await;
        let config_repo = SqliteSystemConfigRepository::new(pool.clone());
        let peer_repo = SqlitePeerRepository::new(pool.clone());
        // 预置一个占用 10.8.0.2 的 peer
        peer_repo
            .insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None, "")
            .await
            .unwrap();
        let subnet: Ipv4Net = "10.8.0.0/24".parse().unwrap();
        let svc = build_peer_service(
            peer_repo,
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        // 新注册（user-2）应跳过已占用的 .2，拿到 .3
        let resp = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_eq!(resp.vpn_ip, "10.8.0.3");
    }

    #[tokio::test]
    async fn force_remove_marks_force_removed_and_blocks_heartbeat() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();

        svc.force_remove(&peer.id).await.unwrap();
        // 已从 control 移除
        assert!(svc.control.list_peers().await.unwrap().is_empty());
        // 状态为 force_removed
        let row = svc.peer_repo.find_by_id(&peer.id).await.unwrap().unwrap();
        assert_eq!(row.status, "force_removed");

        // 心跳被拒（TokenExpired → 401）
        let err = svc
            .heartbeat_checked("user-1", None, 1000)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::TokenExpired));
    }

    #[tokio::test]
    async fn force_remove_then_reregister_recovers_online() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();

        // 强制下线 → 心跳被拒。
        svc.force_remove(&peer.id).await.unwrap();
        assert!(matches!(
            svc.heartbeat_checked("user-1", None, 1000).await.unwrap_err(),
            AppError::TokenExpired
        ));

        // 重新注册(踢下线后重连)→ 解除 force_removed,状态回 offline。
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let row = svc.peer_repo.find_by_id(&peer.id).await.unwrap().unwrap();
        assert_eq!(row.status, "offline");

        // 心跳恢复 → online。
        svc.heartbeat_checked("user-1", None, 2000).await.unwrap();
        let row = svc.peer_repo.find_by_id(&peer.id).await.unwrap().unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn force_remove_unknown_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.force_remove("missing").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn purge_deletes_row_removes_control_and_frees_ip() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        let first_ip = r1.vpn_ip.clone();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();

        svc.purge(&peer.id).await.unwrap();
        // 从 control 移除
        assert!(svc.control.list_peers().await.unwrap().is_empty());
        // 库行已物理删除
        assert!(svc.peer_repo.find_by_id(&peer.id).await.unwrap().is_none());
        // IP 已回收：新用户注册应复用同一 IP（而非顺延到下一个）
        let r2 = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_eq!(r2.vpn_ip, first_ip);
    }

    #[tokio::test]
    async fn purge_after_force_remove_succeeds() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        // 先强制下线，再彻底删除：WG 摘除为 best-effort，不应报错
        svc.force_remove(&peer.id).await.unwrap();
        svc.purge(&peer.id).await.unwrap();
        assert!(svc.peer_repo.find_by_id(&peer.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn purge_unknown_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.purge("missing").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn update_peer_routes_persists_and_validates() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        // 合法网段（带归一化）。
        svc.update_peer_routes(&peer.id, &["192.168.10.5/24".to_string()])
            .await
            .unwrap();
        let updated = svc.peer_repo.find_by_id(&peer.id).await.unwrap().unwrap();
        assert_eq!(updated.routed_subnets, "192.168.10.0/24");
        // 非法网段被拒。
        let err = svc
            .update_peer_routes(&peer.id, &["bad".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
        // 未知 peer。
        let err = svc.update_peer_routes("missing", &[]).await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn heartbeat_checked_ok_for_normal_peer() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.heartbeat_checked("user-1", Some("1.2.3.4:99"), 1000)
            .await
            .unwrap();
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn list_admin_peers_returns_username() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let query = vpn_api_types::peer::AdminPeerQuery {
            page: None,
            page_size: None,
            search: None,
            status: None,
        };
        let page = svc.list_admin_peers(&query).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].username, "alice");
    }

    #[tokio::test]
    async fn scan_offline_marks_stale_peers() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        // last_seen far in past
        svc.heartbeat("user-1", None, 100).await.unwrap();
        let marked = svc
            .scan_offline(100 + OFFLINE_THRESHOLD_MS + 1)
            .await
            .unwrap();
        assert_eq!(marked, 1);
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "offline");
    }
}
