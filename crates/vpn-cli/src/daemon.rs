//! Story 4.14: daemon 主循环。
//!
//! daemon 负责持有 VPN 连接的生命周期：
//! 1. 读凭证（server_url + refresh_token）→ API refresh/login 拿 access token；
//! 2. `generate_keypair` → `register_peer` 取得分配的 vpn_ip 与服务端信息；
//! 3. [`bring_up_tunnel`] 启动**用户态 WireGuard 数据面**（见 [`crate::wg_userspace`]：
//!    boringtun + tun + UDP，全平台统一、零外部依赖，仅需 root/管理员开 TUN）；
//! 4. 启动心跳任务（每 30s `heartbeat`）；
//! 5. 启动 IPC server 处理 CLI 的 Connect/Disconnect/GetStatus；
//! 6. Disconnect 时一个 watch 信号同时停心跳与转发任务（后者自行删路由、关设备）。
//!
//! 状态机 [`ConnState`]：Disconnected / Connecting / Connected / Reconnecting /
//! Error，经共享 [`SharedState`] 对外（IPC）暴露。
//!
//! 单测覆盖**纯逻辑**（状态转移、心跳间隔常量、allowed-ips 计算、CIDR 推断）；
//! 真正建隧道需 root + TUN 设备 + 真实对端，在真机/容器验证。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::api::ApiClient;
use crate::config::DaemonConfig;
use crate::error::{CliError, CliResult};
use crate::ipc::{ConnState, IpcRequest, IpcResponse, StatusResponse};

/// 心跳间隔（秒）。服务端期望 30s 一次（见 vpn_api_types::peer）。
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// 丢包率统计窗口：最近 N 次心跳的成败样本（约 10 分钟）。
pub const HEARTBEAT_LOSS_WINDOW: usize = 20;
/// 上报丢包率所需的最少样本数（不足时不上报，避免头几拍噪声）。
pub const HEARTBEAT_LOSS_MIN_SAMPLES: usize = 5;

/// WireGuard persistent-keepalive（秒），用于 NAT 保活（穿透常驻 NAT 映射）。
pub const PERSISTENT_KEEPALIVE_SECS: u16 = 25;

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
    /// 应导入隧道的网段（AllowedIPs）：VPN 子网 + 各站点 LAN。
    pub allowed_routes: Vec<String>,
}

/// 纯逻辑：计算客户端隧道实际使用的 allowed-ips。
///
/// 服务端 `allowed_routes` 已含 VPN 子网；为兼容旧服务端（字段缺省为空），
/// 空时回退为仅 VPN 子网。
pub fn effective_allowed_ips(params: &TunnelParams) -> Vec<String> {
    if params.allowed_routes.is_empty() {
        vec![params.vpn_subnet.clone()]
    } else {
        params.allowed_routes.clone()
    }
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

/// 执行一次连接：login/refresh → register，返回隧道参数。
///
/// 仅负责控制平面（鉴权 + 注册取得分配的 VPN IP 与服务端信息）；真正建立数据面
/// 隧道由 [`bring_up_tunnel`] 承担，心跳由 [`run_heartbeat`] 承担。
pub async fn connect_once(
    api: &ApiClient,
    keypair: &vpn_wireguard::WgKeypair,
    device_name: &str,
    routed_subnets: &[String],
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
        // 站点网关模式：登录时 `--route` 声明的 LAN 网段（经 DaemonConfig 传入）。
        routed_subnets: routed_subnets.to_vec(),
        // 节点健康监控：上报客户端版本。
        client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let resp = api.register_peer(&req).await?;

    // Split-horizon：内网节点可经 `VPN_ENDPOINT_OVERRIDE` 用内网 endpoint 直连，
    // 避开公网 IP 的 NAT 回环；外网节点不设置该变量，用服务端下发的(公网) endpoint。
    let server_endpoint = std::env::var("VPN_ENDPOINT_OVERRIDE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| resp.server_endpoint.clone());
    if server_endpoint != resp.server_endpoint {
        tracing::info!(override_endpoint = %server_endpoint, server_endpoint = %resp.server_endpoint, "使用 VPN_ENDPOINT_OVERRIDE 覆盖隧道 endpoint");
    }

    Ok(TunnelParams {
        vpn_ip: resp.vpn_ip.clone(),
        vpn_subnet: resp.vpn_subnet.clone(),
        server_public_key: resp.server_public_key.clone(),
        server_endpoint,
        client_private_key: keypair.private_key.clone(),
        allowed_routes: resp.allowed_routes.clone(),
    })
}

/// 建立客户端数据面隧道：**全平台统一走用户态 WireGuard**（boringtun，零外部依赖）。
///
/// 不再 shell-out 到 `wg`/`wg-quick`/`wireguard.exe`——隧道在进程内完成（见
/// [`crate::wg_userspace`]）。仅需 root/管理员开 TUN 设备。转发任务在 `shutdown`
/// 置位后自行清理（删路由、关设备），故 [`tear_down_tunnel`] 只需触发该信号。
pub async fn bring_up_tunnel(
    iface: &str,
    keypair: &vpn_wireguard::WgKeypair,
    params: &TunnelParams,
    shutdown: tokio::sync::watch::Receiver<bool>,
    // 转发循环遇致命错误时广播关停(连带停心跳)的发送端,通常传 shutdown 对应 Sender 的 clone。
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    // 流量计数回写目标（前端读 bytes_rx/bytes_tx）；None 时不统计。
    traffic: Option<SharedState>,
    // 实时路由更新接收端(P1.4);None 时不支持热更新。
    routes_rx: Option<tokio::sync::watch::Receiver<Vec<String>>>,
) -> CliResult<tokio::task::JoinHandle<CliResult<()>>> {
    let vpn_ip: std::net::Ipv4Addr = params
        .vpn_ip
        .parse()
        .map_err(|e| CliError::Other(format!("非法 vpn_ip `{}`: {e}", params.vpn_ip)))?;
    // 从 vpn_subnet（如 10.8.0.0/24）取前缀；缺省按 /24。
    let prefix = params
        .vpn_subnet
        .split_once('/')
        .and_then(|(_, p)| p.parse::<u8>().ok())
        .unwrap_or(24);
    let allowed = effective_allowed_ips(params);
    crate::wg_userspace::UserspaceTunnel::bring_up(
        iface,
        &keypair.private_key,
        &params.server_public_key,
        &params.server_endpoint,
        vpn_ip,
        prefix,
        &allowed,
        PERSISTENT_KEEPALIVE_SECS,
        shutdown,
        shutdown_tx,
        traffic,
        routes_rx,
    )
    .await
}

/// 拆除隧道：用户态实现的转发任务在 shutdown 信号后自行删路由 + 关设备，
/// 故此处无需额外动作（保留函数以维持调用点语义清晰）。
pub async fn tear_down_tunnel(_iface: &str) {}

/// 心跳循环：每 [`HEARTBEAT_INTERVAL_SECS`] 秒上报一次，**韧性重连**版。
///
/// 关键语义：网络抖动不应拆隧道——用户态数据面(boringtun)的定时器会自动重握手，
/// 只要心跳任务不"死掉"。因此：
/// - **瞬时失败**（网络中断/服务端暂不可达）：不退出，标记 [`ConnState::Reconnecting`]，
///   下个 tick 重试；成功后自动恢复 [`ConnState::Connected`]。
/// - **token 彻底失效**（access 已自动刷新仍失败 → 被管理员强制下线）：返回 `Err`，
///   交由上层拆隧道并要求重新登录。
///
/// `state` 用于回写重连/恢复状态(前端据此显示)；`None` 时仅靠返回值表达结果。
///
/// P1.4:心跳响应回带服务端实时计算的 `allowed_routes`;与 `initial_routes`(建隧道时
/// 的集合)比对,有变化即经 `routes_tx` 下发给转发循环做增量路由更新——组/网段变更
/// 无需重连即对在线节点生效。比对顺序无关(排序后比)。
#[allow(clippy::too_many_arguments)]
pub async fn run_heartbeat(
    api: Arc<ApiClient>,
    endpoint: Option<String>,
    // 本机 WireGuard 公钥：多终端模式下服务端据此精确定位是哪台终端在打卡。
    wg_public_key: Option<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    state: Option<SharedState>,
    initial_routes: Vec<String>,
    routes_tx: Option<tokio::sync::watch::Sender<Vec<String>>>,
) -> CliResult<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let mut failures: u32 = 0;
    // 节点健康监控：最近一次心跳 RTT + 最近 N 次心跳成败窗口（用于丢包率上报）。
    let mut last_rtt_ms: Option<i64> = None;
    let mut samples: std::collections::VecDeque<bool> =
        std::collections::VecDeque::with_capacity(HEARTBEAT_LOSS_WINDOW);
    // 当前已应用的 allowed_routes(排序后,用于顺序无关比对)。
    let mut current_sorted = {
        let mut v = initial_routes;
        v.sort();
        v
    };
    loop {
        tokio::select! {
            res = shutdown.changed() => {
                // sender 被 drop（主循环退出/连接被替换）或显式置位 true → 退出心跳循环，
                // 不在通道关闭后空转（changed() 对已关闭通道会立即 Err 并永久就绪）。
                if res.is_err() || *shutdown.borrow() { break; }
            }
            _ = ticker.tick() => {
                // 样本不足时不上报丢包率，避免头几拍的 0%/100% 噪声。
                let loss_pct = if samples.len() >= HEARTBEAT_LOSS_MIN_SAMPLES {
                    let lost = samples.iter().filter(|ok| !**ok).count();
                    Some(lost as f64 * 100.0 / samples.len() as f64)
                } else {
                    None
                };
                let req = vpn_api_types::peer::PeerHeartbeatRequest {
                    endpoint: endpoint.clone(),
                    wg_public_key: wg_public_key.clone(),
                    rtt_ms: last_rtt_ms,
                    loss_pct,
                };
                let started = std::time::Instant::now();
                let result = api.heartbeat(&req).await;
                if samples.len() >= HEARTBEAT_LOSS_WINDOW {
                    samples.pop_front();
                }
                samples.push_back(result.is_ok());
                match result {
                    Ok(resp) => {
                        last_rtt_ms = Some(started.elapsed().as_millis() as i64);
                        tracing::info!(
                            rtt_ms = last_rtt_ms.unwrap_or_default(),
                            routes = resp.allowed_routes.len(),
                            "VPN 心跳成功"
                        );
                        // P1.4:检测 allowed_routes 变化 → 下发实时路由更新。
                        let mut next_sorted = resp.allowed_routes.clone();
                        next_sorted.sort();
                        // 空集视为“服务端未回带路由信息”（旧服务端 data:null → 解析为默认空）：
                        // 跳过下发，避免把本地路由全删成黑洞。新服务端的 allowed_routes 恒含
                        // VPN 子网，不会为空。
                        if !resp.allowed_routes.is_empty() && next_sorted != current_sorted {
                            tracing::info!(
                                routes = ?resp.allowed_routes,
                                "检测到 allowed_routes 变更,下发实时路由更新"
                            );
                            current_sorted = next_sorted;
                            if let Some(tx) = &routes_tx {
                                let _ = tx.send(resp.allowed_routes);
                            }
                        }
                        // 抖动后恢复:从 Reconnecting 标回 Connected。
                        if failures > 0 {
                            failures = 0;
                            tracing::info!("心跳恢复,连接已重新建立");
                            if let Some(s) = &state {
                                s.set_state(ConnState::Connected, now_unix()).await;
                            }
                        }
                    }
                    // 致命错误 → 交上层拆隧道、回登录，而非当瞬时错误无限重连、把死隧道挂着黑洞流量：
                    // - token+refresh 都失效（被管理员强制下线）；
                    // - 服务端已无此 peer（被管理员彻底删除）→ PeerNotFound。
                    Err(e) if e.is_token_expired() || e.is_peer_gone() => return Err(e),
                    // 瞬时网络错误:保持隧道,标记重连,下个 tick 再试。
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        tracing::warn!(error = %e.safe_diagnostic(), failures, "心跳失败,保持隧道并重试");
                        if let Some(s) = &state {
                            s.set_state(ConnState::Reconnecting, now_unix()).await;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// 同时监督数据面与心跳任务。内部故障广播 shutdown 并进入 Error；主动停止只记录退出。
pub async fn supervise_connection_tasks(
    mut forward: tokio::task::JoinHandle<CliResult<()>>,
    mut heartbeat: tokio::task::JoinHandle<CliResult<()>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    stop_requested: Arc<AtomicBool>,
    shared: SharedState,
) {
    tokio::select! {
        result = &mut forward => {
            let requested = stop_requested.load(Ordering::Acquire);
            report_connection_task("数据面", result, requested, &shared).await;
            let _ = shutdown_tx.send(true);
            await_connection_peer("心跳", &mut heartbeat).await;
        }
        result = &mut heartbeat => {
            let requested = stop_requested.load(Ordering::Acquire);
            report_connection_task("心跳", result, requested, &shared).await;
            let _ = shutdown_tx.send(true);
            await_connection_peer("数据面", &mut forward).await;
        }
    }
}

async fn await_connection_peer(task: &str, handle: &mut tokio::task::JoinHandle<CliResult<()>>) {
    match handle.await {
        Ok(Ok(())) => tracing::info!(task, "关联 VPN 任务已停止"),
        Ok(Err(error)) => {
            tracing::warn!(task, error = %error.safe_diagnostic(), "关联 VPN 任务返回错误")
        }
        Err(error) => {
            let error = crate::error::redact_sensitive(&error.to_string());
            tracing::warn!(task, %error, "关联 VPN 任务 panic")
        }
    }
}

async fn report_connection_task(
    task: &str,
    result: Result<CliResult<()>, tokio::task::JoinError>,
    stop_requested: bool,
    shared: &SharedState,
) {
    if stop_requested {
        match result {
            Ok(Ok(())) => tracing::info!(task, "VPN 后台任务按请求退出"),
            Ok(Err(error)) => {
                tracing::warn!(task, error = %error.safe_diagnostic(), "VPN 后台任务关停时返回错误")
            }
            Err(error) => {
                let error = crate::error::redact_sensitive(&error.to_string());
                tracing::warn!(task, %error, "VPN 后台任务关停时 panic")
            }
        }
        return;
    }

    let message = match result {
        Ok(Ok(())) => format!("{task}任务意外提前退出"),
        Ok(Err(error)) if error.is_token_expired() => {
            "已被管理员强制下线,请重新登录后再连接".to_string()
        }
        Ok(Err(error)) => format!("{task}任务失败:{}", error.safe_diagnostic()),
        Err(error) if error.is_panic() => format!(
            "{task}任务 panic:{}",
            crate::error::redact_sensitive(&error.to_string())
        ),
        Err(error) => format!(
            "{task}任务异常结束:{}",
            crate::error::redact_sensitive(&error.to_string())
        ),
    };
    tracing::error!(task, error = %message, "VPN 后台任务异常退出");
    shared.set_error(message, now_unix()).await;
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

    // 主循环：响应控制消息。Connect → 注册 + 建内核隧道 + 启动心跳任务；
    // Disconnect → 停心跳 + 拆隧道。保证不 panic 且可被 Disconnect 优雅关停。
    let keypair = vpn_wireguard::generate_keypair();
    // 每条活跃连接持有一个心跳任务关停信号；重连/断开时替换或清空。
    let mut active_shutdown: Option<tokio::sync::watch::Sender<bool>> = None;
    // supervisor 持有转发与心跳任务；重连前必须等它完成路由清理。
    let mut active_supervisor: Option<tokio::task::JoinHandle<()>> = None;
    let mut active_stop_requested: Option<Arc<AtomicBool>> = None;
    'control: while let Some(msg) = ctrl_rx.recv().await {
        match msg {
            ControlMsg::Connect => {
                state.set_state(ConnState::Connecting, now_unix()).await;
                match connect_once(&api, &keypair, &config.device_name, &config.routed_subnets)
                    .await
                {
                    Ok(params) => {
                        // 注册成功后再拆旧连接（先提交后拆）：避免一次冗余 Connect 叠加瞬时失败
                        // 把正在工作的隧道毁掉。先发关停信号，再**等待**旧转发任务退出（它会删自己
                        // 加的路由）——否则旧任务的 `route delete` 可能删掉下面新隧道刚加的同 CIDR
                        // 路由，导致重连黑洞；固定 TUN 名也可能因旧设备未释放而 EBUSY。
                        if let Some(requested) = active_stop_requested.take() {
                            requested.store(true, Ordering::Release);
                        }
                        if let Some(tx) = active_shutdown.take() {
                            let _ = tx.send(true);
                        }
                        if let Some(mut old) = active_supervisor.take() {
                            match tokio::time::timeout(Duration::from_secs(5), &mut old).await {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => {
                                    let error = crate::error::redact_sensitive(&error.to_string());
                                    state
                                        .set_error(format!("旧 VPN 任务异常: {error}"), now_unix())
                                        .await;
                                    continue 'control;
                                }
                                Err(_) => {
                                    active_supervisor = Some(old);
                                    state
                                        .set_error("旧 VPN 任务停止超时，已阻止重连", now_unix())
                                        .await;
                                    continue 'control;
                                }
                            }
                        }
                        // 关停信号：隧道转发任务与心跳任务共用，Disconnect 一并停止。
                        let (sd_tx, sd_rx) = tokio::sync::watch::channel(false);
                        // 实时路由通道(P1.4):心跳→转发循环下发新 allowed_routes。
                        let init_routes = effective_allowed_ips(&params);
                        let (routes_tx, routes_rx) =
                            tokio::sync::watch::channel(init_routes.clone());
                        match bring_up_tunnel(
                            &config.interface,
                            &keypair,
                            &params,
                            sd_rx.clone(),
                            sd_tx.clone(),
                            Some(state.clone()),
                            Some(routes_rx),
                        )
                        .await
                        {
                            Ok(forward) => {
                                state.set_vpn_ip(Some(params.vpn_ip.clone())).await;
                                state.set_state(ConnState::Connected, now_unix()).await;
                                // 启动心跳任务：每 30s 上报，daemon 在线即自动保活。
                                // 韧性重连:瞬时失败不退出;仅致命错误(强制下线/peer 被删)才退出并置错误态。
                                let api_hb = api.clone();
                                let state_hb = state.clone();
                                let hb_pubkey = keypair.public_key.clone();
                                let heartbeat = tokio::spawn(async move {
                                    run_heartbeat(
                                        api_hb,
                                        None,
                                        Some(hb_pubkey),
                                        sd_rx,
                                        Some(state_hb.clone()),
                                        init_routes,
                                        Some(routes_tx),
                                    )
                                    .await
                                });
                                let stop_requested = Arc::new(AtomicBool::new(false));
                                let supervisor = tokio::spawn(supervise_connection_tasks(
                                    forward,
                                    heartbeat,
                                    sd_tx.clone(),
                                    stop_requested.clone(),
                                    state.clone(),
                                ));
                                active_shutdown = Some(sd_tx);
                                active_stop_requested = Some(stop_requested);
                                active_supervisor = Some(supervisor);
                                tracing::info!(vpn_ip = %params.vpn_ip, iface = %config.interface, "已连接，心跳已启动");
                            }
                            Err(e) => {
                                // 建隧道失败：sd_tx 随作用域 drop（未 spawn 任务）。
                                state.set_error(e.to_string(), now_unix()).await;
                            }
                        }
                    }
                    Err(e) => {
                        state.set_error(e.to_string(), now_unix()).await;
                    }
                }
            }
            ControlMsg::Disconnect => {
                if let Some(requested) = active_stop_requested.take() {
                    requested.store(true, Ordering::Release);
                }
                if let Some(tx) = active_shutdown.take() {
                    let _ = tx.send(true);
                }
                if let Some(mut old) = active_supervisor.take() {
                    match tokio::time::timeout(Duration::from_secs(5), &mut old).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            let error = crate::error::redact_sensitive(&error.to_string());
                            state
                                .set_error(format!("VPN 后台任务异常: {error}"), now_unix())
                                .await;
                            continue 'control;
                        }
                        Err(_) => {
                            active_supervisor = Some(old);
                            state.set_error("VPN 后台任务停止超时", now_unix()).await;
                            continue 'control;
                        }
                    }
                }
                tear_down_tunnel(&config.interface).await;
                state.set_state(ConnState::Disconnected, now_unix()).await;
            }
        }
    }
    Ok(())
}

pub(crate) fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_reports_unexpected_completion() {
        let shared = SharedState::new();
        report_connection_task("数据面", Ok(Ok(())), false, &shared).await;
        let status = shared.snapshot().await;
        assert_eq!(status.state, ConnState::Error);
        assert_eq!(status.last_error.as_deref(), Some("数据面任务意外提前退出"));
    }

    #[tokio::test]
    async fn supervisor_ignores_requested_completion() {
        let shared = SharedState::new();
        report_connection_task("数据面", Ok(Ok(())), true, &shared).await;
        let status = shared.snapshot().await;
        assert_eq!(status.state, ConnState::Disconnected);
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn supervisor_redacts_panic_payload() {
        let shared = SharedState::new();
        let result = tokio::spawn(async { panic!("token=synthetic-secret") }).await;
        report_connection_task("心跳", result.map(|_| Ok(())), false, &shared).await;
        let message = shared.snapshot().await.last_error.unwrap();
        assert!(message.contains("[REDACTED sensitive diagnostic]"));
        assert!(!message.contains("synthetic-secret"));
    }

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

    fn sample_params(allowed: Vec<String>) -> TunnelParams {
        TunnelParams {
            vpn_ip: "10.8.0.5".into(),
            vpn_subnet: "10.8.0.0/24".into(),
            server_public_key: "spub".into(),
            server_endpoint: "1.2.3.4:51820".into(),
            client_private_key: "priv".into(),
            allowed_routes: allowed,
        }
    }

    #[test]
    fn effective_allowed_ips_uses_server_routes_when_present() {
        let p = sample_params(vec!["10.8.0.0/24".into(), "192.168.20.0/24".into()]);
        assert_eq!(
            effective_allowed_ips(&p),
            vec!["10.8.0.0/24".to_string(), "192.168.20.0/24".to_string()]
        );
    }

    #[test]
    fn effective_allowed_ips_falls_back_to_vpn_subnet() {
        let p = sample_params(vec![]);
        assert_eq!(effective_allowed_ips(&p), vec!["10.8.0.0/24".to_string()]);
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
