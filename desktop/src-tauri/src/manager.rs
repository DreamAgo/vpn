//! 进程内 VPN 连接管理器:**单进程、库调用**,不再依赖独立 daemon / IPC。
//!
//! 直接把 `vpn-cli` 当库用:`connect_once`(注册)→ `bring_up_tunnel`(用户态
//! boringtun 隧道,见 `vpn_cli::wg_userspace`)→ `run_heartbeat`,全部跑在本 GUI
//! 进程里。连接状态用 `vpn_cli::daemon::SharedState` 维护,直接喂给前端。
//!
//! 代价:开 TUN 设备需要 root/管理员,所以**整个 App 需以特权运行**
//! (macOS `sudo`、Windows 管理员)。这是单进程方案的固有要求。

use std::sync::Arc;

use tokio::sync::{watch, Mutex};
use vpn_cli::api::ApiClient;
use vpn_cli::config::{default_device_name, CredentialRepo, DEFAULT_INTERFACE};
use vpn_cli::daemon::{self, SharedState};
use vpn_cli::ipc::{ConnState, StatusResponse};
use vpn_wireguard::{generate_keypair, WgKeypair};

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 单进程连接管理器(放进 Tauri 托管状态,以 `Arc` 共享给命令与托盘)。
pub struct VpnManager {
    /// 本机 WireGuard 密钥对(启动生成一次,重连复用→服务端分配同一 VPN IP)。
    keypair: WgKeypair,
    /// 对外暴露的连接状态(前端轮询读取)。
    shared: SharedState,
    /// 当前连接的关停信号发送端(隧道转发任务 + 心跳任务共用);None 表示未连接。
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    /// 当前连接的转发任务句柄;重连时等它清完路由再建新隧道(避免删掉新隧道的同 CIDR 路由)。
    forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// TUN 接口名。
    iface: String,
}

impl VpnManager {
    pub fn new() -> Self {
        Self {
            keypair: generate_keypair(),
            shared: SharedState::new(),
            shutdown: Mutex::new(None),
            forward: Mutex::new(None),
            iface: std::env::var("VPN_CLI_INTERFACE")
                .unwrap_or_else(|_| DEFAULT_INTERFACE.to_string()),
        }
    }

    /// 当前状态快照。
    pub async fn status(&self) -> StatusResponse {
        self.shared.snapshot().await
    }

    /// 建立连接:注册 → 建用户态隧道 → 启动心跳。需特权(开 TUN)。
    pub async fn connect(&self) -> Result<(), String> {
        let repo = CredentialRepo::file().map_err(|e| e.to_string())?;
        let server = repo
            .server_url()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "未登录:请先登录".to_string())?;
        let refresh = repo
            .refresh_token()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "未登录:请先登录".to_string())?;
        let routes = repo.routes().map_err(|e| e.to_string())?;

        let api = Arc::new(ApiClient::new(&server).map_err(|e| e.to_string())?);
        api.set_refresh_token(refresh);
        let device = default_device_name();

        self.shared.set_state(ConnState::Connecting, now_unix()).await;

        // 1) 控制平面:注册取 vpn_ip + allowed_routes + 服务端 endpoint。
        // **先注册再拆旧隧道**:托盘 Connect 始终可点,一次冗余点击叠加瞬时网络/服务端
        // 错误不应毁掉正在工作的连接。注册失败时旧隧道原封不动。
        let params = match daemon::connect_once(&api, &self.keypair, &device, &routes).await {
            Ok(p) => p,
            Err(e) => {
                self.shared.set_error(e.to_string(), now_unix()).await;
                return Err(e.to_string());
            }
        };

        // 注册成功后再停掉旧连接,并**等待**旧转发任务退出(它会删自己加的路由)——否则旧任务
        // 的 route delete 可能删掉下面新隧道刚加的同 CIDR 路由,导致重连黑洞。
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(true);
        }
        if let Some(old) = self.forward.lock().await.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), old).await;
        }

        // 2) 数据面:用户态 boringtun 隧道(本进程内开 TUN + 加路由 + 转发循环)。
        let (tx, rx) = watch::channel(false);
        // 实时路由通道(P1.4):心跳检测到 allowed_routes 变化 → 转发循环增量更新路由。
        let init_routes = daemon::effective_allowed_ips(&params);
        let (routes_tx, routes_rx) = watch::channel(init_routes.clone());
        let task = match daemon::bring_up_tunnel(
            &self.iface,
            &self.keypair,
            &params,
            rx.clone(),
            tx.clone(),
            Some(self.shared.clone()),
            Some(routes_rx),
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                self.shared.set_error(e.to_string(), now_unix()).await;
                return Err(e.to_string());
            }
        };
        *self.forward.lock().await = Some(task);

        self.shared.set_vpn_ip(Some(params.vpn_ip.clone())).await;
        self.shared.set_state(ConnState::Connected, now_unix()).await;
        let hb_tx = tx.clone();
        *self.shutdown.lock().await = Some(tx);

        // 3) 心跳:每 30s 上报,**韧性重连**。网络抖动不拆隧道——run_heartbeat 内部标记
        //    Reconnecting 并重试,boringtun 自动重握手,恢复后回 Connected。只有被管理员
        //    强制下线(token 彻底失效)才返回 Err → 拆隧道 + 写错误状态,前端据此回登录页。
        let api_hb = api.clone();
        let shared = self.shared.clone();
        let hb_state = shared.clone();
        let hb_pubkey = self.keypair.public_key.clone();
        tokio::spawn(async move {
            if let Err(e) = daemon::run_heartbeat(
                api_hb,
                None,
                Some(hb_pubkey),
                rx,
                Some(hb_state),
                init_routes,
                Some(routes_tx),
            )
            .await
            {
                let _ = hb_tx.send(true); // 停掉数据面转发任务(拆隧道、删路由)
                let msg = if e.is_token_expired() {
                    // 约定:含"强制下线"→ 前端识别为被踢、需重新登录。
                    "已被管理员强制下线,请重新登录后再连接".to_string()
                } else {
                    format!("连接已断开:{e}")
                };
                shared.set_error(msg, now_unix()).await;
            }
        });

        Ok(())
    }

    /// 断开:发关停信号(隧道转发任务自行删路由/关设备,心跳停止)。
    pub async fn disconnect(&self) -> Result<(), String> {
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(true);
        }
        if let Some(old) = self.forward.lock().await.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), old).await;
        }
        self.shared
            .set_state(ConnState::Disconnected, now_unix())
            .await;
        Ok(())
    }
}

impl Default for VpnManager {
    fn default() -> Self {
        Self::new()
    }
}
