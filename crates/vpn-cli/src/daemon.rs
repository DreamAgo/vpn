//! Story 4.14: daemon 主循环。
//!
//! daemon 负责持有 VPN 连接的生命周期：
//! 1. 读凭证（server_url + refresh_token）→ API refresh/login 拿 access token；
//! 2. `generate_keypair` → `register_peer` 取得分配的 vpn_ip 与服务端信息；
//! 3. `open_tun` + `configure_ip(vpn_ip)` 打开并配置 TUN 设备；
//! 4. 启动心跳任务（每 30s `heartbeat`）；
//! 5. 启动 IPC server 处理 CLI 的 Connect/Disconnect/GetStatus；
//! 6. 数据面转发循环（TUN <-> WireGuard 加解密 <-> UDP）——**骨架**，真机验证。
//!
//! 状态机 [`ConnState`]：Disconnected / Connecting / Connected / Reconnecting /
//! Error，经共享 [`SharedState`] 对外（IPC）暴露。
//!
//! 本模块大量路径需要真实设备 / root / 网络，单测覆盖**纯逻辑**（状态转移、
//! 心跳间隔常量、endpoint 推断），系统集成路径以 `// 真机验证` 注释标注且
//! 不 panic。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::api::ApiClient;
use crate::config::DaemonConfig;
use crate::error::{CliError, CliResult};
use crate::ipc::{ConnState, IpcRequest, IpcResponse, StatusResponse};

/// 心跳间隔（秒）。服务端期望 30s 一次（见 vpn_api_types::peer）。
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// daemon 共享状态（IPC 与后台任务并发访问）。
#[derive(Debug, Clone)]
pub struct SharedState {
    inner: Arc<Mutex<StatusResponse>>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    /// 初始 Disconnected 状态。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StatusResponse::disconnected())),
        }
    }

    /// 读取当前状态快照。
    pub async fn snapshot(&self) -> StatusResponse {
        self.inner.lock().await.clone()
    }

    /// 设置连接状态（并维护 since / last_error 的一致性）。
    pub async fn set_state(&self, state: ConnState, now_unix: i64) {
        let mut s = self.inner.lock().await;
        apply_state_transition(&mut s, state, now_unix);
    }

    /// 设置已分配的 VPN IP。
    pub async fn set_vpn_ip(&self, ip: Option<String>) {
        self.inner.lock().await.vpn_ip = ip;
    }

    /// 记录错误信息并进入 Error 态。
    pub async fn set_error(&self, message: impl Into<String>, now_unix: i64) {
        let mut s = self.inner.lock().await;
        s.last_error = Some(message.into());
        apply_state_transition(&mut s, ConnState::Error, now_unix);
    }

    /// 累加流量计数（数据面转发循环调用）。
    pub async fn add_traffic(&self, rx: u64, tx: u64) {
        let mut s = self.inner.lock().await;
        s.bytes_rx = s.bytes_rx.saturating_add(rx);
        s.bytes_tx = s.bytes_tx.saturating_add(tx);
    }
}

/// 纯逻辑：应用状态转移并维护派生字段。
///
/// - 进入 `Connected` 时设置 `since`（若尚未设置）。
/// - 离开 `Connected`（到 Disconnected/Error）时清空 `since` 与流量计数。
/// - 进入非 Error 态时清空 `last_error`。
pub fn apply_state_transition(s: &mut StatusResponse, next: ConnState, now_unix: i64) {
    match next {
        ConnState::Connected => {
            if s.since.is_none() {
                s.since = Some(now_unix);
            }
            s.last_error = None;
        }
        ConnState::Disconnected => {
            s.since = None;
            s.bytes_rx = 0;
            s.bytes_tx = 0;
            s.last_error = None;
            s.vpn_ip = None;
        }
        ConnState::Connecting | ConnState::Reconnecting => {
            // 重连时保留 vpn_ip（静态绑定，重连不变），清 since。
            s.since = None;
            if next == ConnState::Connecting {
                s.last_error = None;
            }
        }
        ConnState::Error => { /* 保留 last_error / vpn_ip 供诊断 */ }
    }
    s.state = next;
}

/// IPC 命令分发：把 [`IpcRequest`] 映射为 [`IpcResponse`]。
///
/// `connect_tx` / `disconnect_tx` 用于唤醒主循环执行实际的连接 / 断开动作；
/// 此处仅记录意图并回执当前状态（实际连接由主循环驱动）。
pub async fn dispatch_request(
    req: IpcRequest,
    state: &SharedState,
    ctrl: &tokio::sync::mpsc::Sender<ControlMsg>,
) -> IpcResponse {
    match req {
        IpcRequest::GetStatus => IpcResponse::Status(state.snapshot().await),
        IpcRequest::Connect => match ctrl.send(ControlMsg::Connect).await {
            Ok(()) => IpcResponse::Ok,
            Err(_) => IpcResponse::Error {
                message: "daemon 主循环不可用".to_string(),
            },
        },
        IpcRequest::Disconnect => match ctrl.send(ControlMsg::Disconnect).await {
            Ok(()) => IpcResponse::Ok,
            Err(_) => IpcResponse::Error {
                message: "daemon 主循环不可用".to_string(),
            },
        },
    }
}

/// 主循环控制消息（IPC -> 主循环）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMsg {
    /// 请求建立连接。
    Connect,
    /// 请求断开连接。
    Disconnect,
}

/// 一次连接尝试的产出：握手 / 注册得到的隧道参数。
#[derive(Debug, Clone)]
pub struct TunnelParams {
    /// 分配的 VPN IP。
    pub vpn_ip: String,
    /// VPN 子网（CIDR）。
    pub vpn_subnet: String,
    /// 服务端公钥。
    pub server_public_key: String,
    /// 服务端 endpoint（host:port）。
    pub server_endpoint: String,
    /// 客户端私钥（本地生成，不上送服务端）。
    pub client_private_key: String,
}

/// 纯逻辑：从 register 响应 + 本地私钥组装隧道参数，并把 vpn_ip 规整为 CIDR。
///
/// 服务端返回的 `vpn_ip` 通常是裸 IP，需要结合 `vpn_subnet` 的前缀长度组成
/// `configure_ip` 所需的 CIDR。
pub fn build_configure_cidr(vpn_ip: &str, vpn_subnet: &str) -> CliResult<String> {
    // 已是 CIDR 直接用。
    if vpn_ip.contains('/') {
        return Ok(vpn_ip.to_string());
    }
    // 从子网取前缀长度（如 10.8.0.0/24 -> 24）。
    let prefix = vpn_subnet
        .split_once('/')
        .and_then(|(_, p)| p.parse::<u8>().ok())
        .ok_or_else(|| CliError::Invalid(format!("无法从子网 `{vpn_subnet}` 解析前缀长度")))?;
    Ok(format!("{vpn_ip}/{prefix}"))
}

/// 执行一次连接：login/refresh → register → 打开并配置 TUN。
///
/// 返回隧道参数；失败返回错误供重连逻辑决策。真正的数据面转发由
/// [`run_data_plane`] 承担。
pub async fn connect_once(
    api: &ApiClient,
    keypair: &vpn_wireguard::WgKeypair,
    device_name: &str,
) -> CliResult<TunnelParams> {
    // 1) 确保有可用 access token：优先用 refresh。
    if api.access_token().is_none() {
        api.refresh().await?;
    }

    // 2) 注册 peer，取得 vpn_ip 与服务端信息。
    let req = vpn_api_types::peer::PeerRegisterRequest {
        wg_public_key: keypair.public_key.clone(),
        device_name: device_name.to_string(),
        os_info: Some(detect_os_info()),
    };
    let resp = api.register_peer(&req).await?;

    let params = TunnelParams {
        vpn_ip: resp.vpn_ip.clone(),
        vpn_subnet: resp.vpn_subnet.clone(),
        server_public_key: resp.server_public_key.clone(),
        server_endpoint: resp.server_endpoint.clone(),
        client_private_key: keypair.private_key.clone(),
    };

    // 3) 打开 TUN 并配置 IP。真机验证：需要 root / 管理员权限。
    let cidr = build_configure_cidr(&params.vpn_ip, &params.vpn_subnet)?;
    let mut tun = vpn_platform::open_tun("vpn-cli0")?;
    tun.configure_ip(&cidr).await?;
    // 设备打开成功后即关闭句柄；真正的转发循环在 run_data_plane 中重新持有。
    // （此处分离便于「连接验证」与「长期转发」职责清晰；真机上应直接复用。）
    tun.close().await?;

    Ok(params)
}

/// 数据面转发循环骨架：TUN <-> WireGuard 加解密 <-> UDP。
///
/// 真机验证：本轮不实现真实 boringtun 隧道（与 vpn-wireguard 的 x25519-dalek
/// 版本约束冲突，且需 root + UDP socket + 真实对端）。这里给出结构清晰的骨架：
/// - 打开 TUN 设备并配置 IP；
/// - 循环 `recv` 出站 IP 包 → （此处应交给 WireGuard 加密后经 UDP 发往服务端）；
/// - 从 UDP 收到密文 → （解密后）`send` 回 TUN。
///
/// 当前实现仅维持设备打开 + 在收到关停信号时退出，不做真实转发，**不 panic**。
pub async fn run_data_plane(
    params: &TunnelParams,
    state: &SharedState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    now_unix: i64,
) -> CliResult<()> {
    let cidr = build_configure_cidr(&params.vpn_ip, &params.vpn_subnet)?;
    let mut tun = vpn_platform::open_tun("vpn-cli0")?;
    tun.configure_ip(&cidr).await?;
    state.set_state(ConnState::Connected, now_unix).await;

    // 真机验证：此处应构造 WireGuard Tunn（client_private_key + server_public_key）
    // 与 UDP socket（连接 server_endpoint），并用 select! 在三个方向间转发：
    //   tun.recv -> tunn.encapsulate -> udp.send
    //   udp.recv -> tunn.decapsulate -> tun.send
    //   tunn.update_timers 周期性维护握手 / keepalive
    // 当前骨架只等待关停信号，避免在无对端 / 无权限环境忙等或 panic。
    let mut buf = vec![0u8; 1500];
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            // 真机验证：读取出站包。无设备能力时该分支不会就绪，仅占位说明数据流向。
            res = tun.recv(&mut buf) => {
                match res {
                    Ok(n) => {
                        // 出站包：加密后经 UDP 送出（骨架未实现）。
                        state.add_traffic(0, n as u64).await;
                    }
                    Err(_) => break, // 设备异常 -> 交给上层重连
                }
            }
        }
    }
    let _ = tun.close().await;
    Ok(())
}

/// 心跳循环骨架：每 [`HEARTBEAT_INTERVAL_SECS`] 秒上报一次。
///
/// 上报失败（如 token 失效已自动刷新仍失败、网络中断）返回错误，交由重连逻辑。
pub async fn run_heartbeat(
    api: Arc<ApiClient>,
    endpoint: Option<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> CliResult<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            _ = ticker.tick() => {
                let req = vpn_api_types::peer::PeerHeartbeatRequest {
                    endpoint: endpoint.clone(),
                };
                api.heartbeat(&req).await?;
            }
        }
    }
    Ok(())
}

/// 推断本机 OS 信息字符串（用于 register 的 os_info）。
pub fn detect_os_info() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// daemon 启动入口：装配 ApiClient + 共享状态 + IPC server。
///
/// 真机验证：需要凭证、网络与设备权限。失败时把错误写入共享状态并返回。
pub async fn run(config: DaemonConfig) -> CliResult<()> {
    let state = SharedState::new();
    let api = Arc::new(ApiClient::new(&config.server_url)?);
    if let Some(rt) = &config.refresh_token {
        api.set_refresh_token(rt.clone());
    } else {
        state
            .set_error("缺少 refresh token，请先登录", now_unix())
            .await;
        return Err(CliError::NotLoggedIn);
    }

    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::channel::<ControlMsg>(8);

    // IPC server：把请求分发到主循环。
    let ipc_state = state.clone();
    let ipc_ctrl = ctrl_tx.clone();
    let socket = config.socket_path.clone();
    tokio::spawn(async move {
        let handler = move |req: IpcRequest| {
            let st = ipc_state.clone();
            let cx = ipc_ctrl.clone();
            async move { dispatch_request(req, &st, &cx).await }
        };
        let _ = crate::ipc::serve(&socket, handler).await;
    });

    // 主循环：响应控制消息。真机验证：实际驱动 connect_once + run_data_plane +
    // run_heartbeat + 重连。此处保证不 panic 且可被 Disconnect 优雅关停。
    let keypair = vpn_wireguard::generate_keypair();
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    while let Some(msg) = ctrl_rx.recv().await {
        match msg {
            ControlMsg::Connect => {
                state.set_state(ConnState::Connecting, now_unix()).await;
                match connect_once(&api, &keypair, &config.device_name).await {
                    Ok(params) => {
                        state.set_vpn_ip(Some(params.vpn_ip.clone())).await;
                        state.set_state(ConnState::Connected, now_unix()).await;
                    }
                    Err(e) => {
                        state.set_error(e.to_string(), now_unix()).await;
                    }
                }
            }
            ControlMsg::Disconnect => {
                let _ = shutdown_tx.send(true);
                state.set_state(ConnState::Disconnected, now_unix()).await;
            }
        }
    }
    Ok(())
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_interval_is_30s() {
        assert_eq!(HEARTBEAT_INTERVAL_SECS, 30);
    }

    #[test]
    fn build_cidr_from_bare_ip_and_subnet() {
        assert_eq!(
            build_configure_cidr("10.8.0.5", "10.8.0.0/24").unwrap(),
            "10.8.0.5/24"
        );
    }

    #[test]
    fn build_cidr_passthrough_when_already_cidr() {
        assert_eq!(
            build_configure_cidr("10.8.0.5/24", "10.8.0.0/24").unwrap(),
            "10.8.0.5/24"
        );
    }

    #[test]
    fn build_cidr_rejects_bad_subnet() {
        assert!(build_configure_cidr("10.8.0.5", "10.8.0.0").is_err());
        assert!(build_configure_cidr("10.8.0.5", "nonsense").is_err());
    }

    #[test]
    fn detect_os_info_nonempty() {
        let s = detect_os_info();
        assert!(s.contains(std::env::consts::OS));
        assert!(s.contains(std::env::consts::ARCH));
    }

    #[test]
    fn transition_connecting_clears_since_keeps_ip() {
        let mut s = StatusResponse {
            state: ConnState::Connected,
            vpn_ip: Some("10.8.0.5".into()),
            since: Some(100),
            bytes_rx: 5,
            bytes_tx: 7,
            last_error: None,
        };
        apply_state_transition(&mut s, ConnState::Reconnecting, 200);
        assert_eq!(s.state, ConnState::Reconnecting);
        assert_eq!(s.since, None);
        assert_eq!(s.vpn_ip, Some("10.8.0.5".into())); // 静态绑定保留
    }

    #[test]
    fn transition_connected_sets_since_and_clears_error() {
        let mut s = StatusResponse {
            state: ConnState::Connecting,
            vpn_ip: Some("10.8.0.5".into()),
            since: None,
            bytes_rx: 0,
            bytes_tx: 0,
            last_error: Some("prev".into()),
        };
        apply_state_transition(&mut s, ConnState::Connected, 500);
        assert_eq!(s.since, Some(500));
        assert_eq!(s.last_error, None);
    }

    #[test]
    fn transition_connected_preserves_existing_since() {
        let mut s = StatusResponse {
            state: ConnState::Connected,
            vpn_ip: None,
            since: Some(42),
            bytes_rx: 0,
            bytes_tx: 0,
            last_error: None,
        };
        apply_state_transition(&mut s, ConnState::Connected, 999);
        assert_eq!(s.since, Some(42)); // 不覆盖
    }

    #[test]
    fn transition_disconnected_resets_everything() {
        let mut s = StatusResponse {
            state: ConnState::Connected,
            vpn_ip: Some("10.8.0.5".into()),
            since: Some(100),
            bytes_rx: 9,
            bytes_tx: 9,
            last_error: Some("e".into()),
        };
        apply_state_transition(&mut s, ConnState::Disconnected, 0);
        assert_eq!(s.state, ConnState::Disconnected);
        assert_eq!(s.since, None);
        assert_eq!(s.bytes_rx, 0);
        assert_eq!(s.bytes_tx, 0);
        assert_eq!(s.vpn_ip, None);
        assert_eq!(s.last_error, None);
    }

    #[test]
    fn transition_error_keeps_diagnostics() {
        let mut s = StatusResponse {
            state: ConnState::Connecting,
            vpn_ip: Some("10.8.0.5".into()),
            since: None,
            bytes_rx: 0,
            bytes_tx: 0,
            last_error: None,
        };
        s.last_error = Some("boom".into());
        apply_state_transition(&mut s, ConnState::Error, 1);
        assert_eq!(s.state, ConnState::Error);
        assert_eq!(s.last_error, Some("boom".into()));
        assert_eq!(s.vpn_ip, Some("10.8.0.5".into()));
    }

    #[tokio::test]
    async fn shared_state_tracks_traffic_and_state() {
        let st = SharedState::new();
        assert_eq!(st.snapshot().await.state, ConnState::Disconnected);
        st.set_state(ConnState::Connecting, 10).await;
        assert_eq!(st.snapshot().await.state, ConnState::Connecting);
        st.set_state(ConnState::Connected, 20).await;
        st.add_traffic(100, 200).await;
        let snap = st.snapshot().await;
        assert_eq!(snap.bytes_rx, 100);
        assert_eq!(snap.bytes_tx, 200);
        assert_eq!(snap.since, Some(20));
    }

    #[tokio::test]
    async fn get_status_dispatch_returns_snapshot() {
        let st = SharedState::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let resp = dispatch_request(IpcRequest::GetStatus, &st, &tx).await;
        match resp {
            IpcResponse::Status(s) => assert_eq!(s.state, ConnState::Disconnected),
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_dispatch_sends_control_msg() {
        let st = SharedState::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let resp = dispatch_request(IpcRequest::Connect, &st, &tx).await;
        assert_eq!(resp, IpcResponse::Ok);
        assert_eq!(rx.recv().await, Some(ControlMsg::Connect));
    }

    #[tokio::test]
    async fn disconnect_dispatch_when_loop_gone_errors() {
        let st = SharedState::new();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx); // 模拟主循环已退出
        let resp = dispatch_request(IpcRequest::Disconnect, &st, &tx).await;
        assert!(matches!(resp, IpcResponse::Error { .. }));
    }
}
