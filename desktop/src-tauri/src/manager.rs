//! VPN 连接管理器。Windows/Linux 由 GUI 持有；macOS 由最小 root helper 持有。
//!
//! 直接把 `vpn-cli` 当库用:`connect_once`(注册)→ `bring_up_tunnel`(用户态
//! boringtun 隧道,见 `vpn_cli::wg_userspace`)→ `run_heartbeat`,全部跑在本 GUI
//! 进程里。连接状态用 `vpn_cli::daemon::SharedState` 维护,直接喂给前端。
//!
//! 代价:开 TUN 设备需要 root/管理员,所以**整个 App 需以特权运行**
//! (macOS `sudo`、Windows 管理员)。这是单进程方案的固有要求。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{watch, Mutex};
use tracing::Instrument;
use vpn_cli::api::ApiClient;
use vpn_cli::cluster::{self, GatewayReconnect, GatewayRetry};
#[cfg(not(target_os = "macos"))]
use vpn_cli::config::{default_device_name, CredentialRepo};
use vpn_cli::config::{DeviceIdentity, DEFAULT_INTERFACE};
use vpn_cli::daemon::{self, SharedState};
use vpn_cli::ipc::{ConnState, StatusResponse};
use vpn_wireguard::WgKeypair;

const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type GatewayMonitor = tokio::task::JoinHandle<Option<GatewayReconnect>>;

#[derive(Clone, Copy)]
struct ConnectionGeneration {
    epoch: u64,
    owner: Option<u64>,
}

struct ActiveGatewayMonitor {
    task: GatewayMonitor,
    owner: u64,
}

struct AttemptError {
    message: String,
    retry_cluster: bool,
}

impl From<String> for AttemptError {
    fn from(message: String) -> Self {
        Self {
            message,
            retry_cluster: true,
        }
    }
}

/// 单进程连接管理器(放进 Tauri 托管状态,以 `Arc` 共享给命令与托盘)。
pub struct VpnManager {
    /// 对外暴露的连接状态(前端轮询读取)。
    shared: SharedState,
    /// 当前连接的关停信号发送端(隧道转发任务 + 心跳任务共用);None 表示未连接。
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    /// 仅由主动断开/重连置位，内部故障广播 shutdown 时保持 false。
    stop_requested: Mutex<Option<Arc<AtomicBool>>>,
    /// 监控转发与心跳任务的 supervisor；异常提前退出会写错误态。
    supervisor: Mutex<Option<tokio::task::JoinHandle<Result<(), String>>>>,
    /// 已确认的数据面清理失败必须保持 fail-closed，禁止后续误报断开成功。
    cleanup_failure: Mutex<Option<String>>,
    /// 串行化连接/断开，防止托盘与窗口并发操作覆盖任务句柄。
    operation: Mutex<()>,
    /// disconnect 在等待 operation 锁之前先递增；正在建连的 manager-owned task
    /// 在每个资源阶段检查，确保超时/会话切换不会迟到建立隧道。
    operation_epoch: AtomicU64,
    /// Replaced only after a new connection has been installed.
    connection_owner: AtomicU64,
    /// TUN 接口名。
    iface: String,
    /// 单调递增的连接尝试编号，仅用于关联本地日志。
    attempt_seq: AtomicU64,
}

impl VpnManager {
    pub fn new() -> Self {
        Self {
            shared: SharedState::new(),
            shutdown: Mutex::new(None),
            stop_requested: Mutex::new(None),
            supervisor: Mutex::new(None),
            cleanup_failure: Mutex::new(None),
            operation: Mutex::new(()),
            operation_epoch: AtomicU64::new(1),
            connection_owner: AtomicU64::new(0),
            iface: std::env::var("VPN_CLI_INTERFACE")
                .unwrap_or_else(|_| DEFAULT_INTERFACE.to_string()),
            attempt_seq: AtomicU64::new(0),
        }
    }

    pub fn subscribe_status(&self) -> watch::Receiver<()> {
        self.shared.subscribe()
    }

    /// 当前状态快照。
    pub async fn status(&self) -> StatusResponse {
        self.shared.snapshot().await
    }

    /// 建立连接:注册 → 建用户态隧道 → 启动心跳。需特权(开 TUN)。
    #[cfg(not(target_os = "macos"))]
    pub async fn connect(self: &Arc<Self>) -> Result<(), String> {
        let repo = CredentialRepo::file().map_err(|error| error.to_string())?;
        let server = repo
            .server_url()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "未登录:请先登录".to_string())?;
        let refresh = repo
            .refresh_token()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "未登录:请先登录".to_string())?;
        let identity = repo
            .device_identity(&server)
            .map_err(|error| error.to_string())?;
        self.connect_with_credentials(server, refresh, default_device_name(), Some(identity))
            .await
    }

    /// 使用调用方提供的短期会话建立连接；macOS helper 不持久化这些值。
    pub async fn connect_with_credentials(
        self: &Arc<Self>,
        server: String,
        refresh: String,
        device: String,
        device_identity: Option<DeviceIdentity>,
    ) -> Result<(), String> {
        let attempt_id = self
            .attempt_seq
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let started = Instant::now();
        let span = tracing::info_span!("vpn_connection", attempt_id);
        // 建连由 manager-owned task 执行。调用方（IPC timeout、console generation
        // 变化）取消等待时，任务仍持有 operation 锁并完成登记；随后 disconnect 必须
        // 等它结束并通过 supervisor 清理，避免 detached TUN/路由迟到删除新连接路由。
        let manager = self.clone();
        let epoch = self.operation_epoch.load(Ordering::Acquire);
        let task = tokio::spawn(
            async move {
                let mut api = ApiClient::new(&server).map_err(|error| error.to_string())?;
                let keypair = if let Some(identity) = device_identity {
                    api = api
                        .with_device_key(identity.device_key())
                        .map_err(|error| error.to_string())?;
                    identity.keypair().map_err(|error| error.to_string())?
                } else {
                    vpn_wireguard::generate_keypair()
                };
                api.set_refresh_token(refresh);
                let api = Arc::new(api);
                let mut retry = GatewayRetry::default();
                let result = manager
                    .connect_attempt(
                        attempt_id,
                        ConnectionGeneration { epoch, owner: None },
                        api.clone(),
                        &keypair,
                        &device,
                        &mut retry,
                    )
                    .await;
                match result {
                    Ok(Some(monitor)) => {
                        tokio::spawn(manager.follow_gateway(
                            ConnectionGeneration {
                                epoch,
                                owner: Some(monitor.owner),
                            },
                            api,
                            keypair,
                            device,
                            retry,
                            Some(monitor.task),
                        ));
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(error) => {
                        let owner = manager.connection_owner.load(Ordering::Acquire);
                        let has_live_connection = manager
                            .supervisor
                            .lock()
                            .await
                            .as_ref()
                            .is_some_and(|task| !task.is_finished());
                        if api.cluster_seen() && error.retry_cluster && !has_live_connection {
                            tokio::spawn(manager.follow_gateway(
                                ConnectionGeneration {
                                    epoch,
                                    owner: Some(owner),
                                },
                                api,
                                keypair,
                                device,
                                retry,
                                None,
                            ));
                        }
                        Err(error.message)
                    }
                }
            }
            .instrument(span.clone()),
        );
        let result: Result<(), String> = task
            .await
            .map_err(|error| format!("VPN 连接任务异常: {error}"))?;
        if let Err(error) = &result {
            let safe = vpn_cli::error::redact_sensitive(error);
            span.in_scope(|| {
                tracing::warn!(
                    stage = "connect",
                    result = "failed",
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %safe,
                    "VPN 连接尝试失败"
                );
            });
        }
        result
    }

    async fn connect_attempt(
        &self,
        attempt_id: u64,
        generation: ConnectionGeneration,
        api: Arc<ApiClient>,
        keypair: &WgKeypair,
        device: &str,
        retry: &mut GatewayRetry,
    ) -> Result<Option<ActiveGatewayMonitor>, AttemptError> {
        let _operation = self.operation.lock().await;
        self.ensure_generation(generation)?;
        let connection_started = Instant::now();
        tracing::info!(attempt_id, iface = %self.iface, stage = "connect", result = "started", "开始建立 VPN 连接");

        // 保留活动连接的 DNS；无隧道时先恢复解析，再刷新 token/注册。
        if self
            .supervisor
            .lock()
            .await
            .as_ref()
            .is_none_or(|task| task.is_finished())
        {
            let dns_started = Instant::now();
            let cleanup = daemon::cleanup_dns_before_connect().await;
            tracing::info!(
                stage = "dns_before_connect",
                result = if cleanup.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                },
                elapsed_ms = dns_started.elapsed().as_millis(),
                "连接前 DNS 清理结束"
            );
            cleanup.map_err(|error| format!("连接前清理 DNS 失败：{error}"))?;
            self.ensure_generation(generation)?;
        }
        let previous_healthy = self.shared.snapshot().await.state == ConnState::Connected;
        if !previous_healthy {
            self.shared
                .set_state(ConnState::Connecting, now_unix())
                .await;
        }

        // 1) 控制平面:注册取 vpn_ip + allowed_routes + 服务端 endpoint。
        // **先注册再拆旧隧道**:托盘 Connect 始终可点,一次冗余点击叠加瞬时网络/服务端
        // 错误不应毁掉正在工作的连接。注册失败时旧隧道原封不动。
        let register_started = Instant::now();
        tracing::info!(
            stage = "register",
            result = "started",
            "开始刷新会话并注册 VPN 节点"
        );
        let params = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            daemon::connect_once_with_retry(&api, keypair, device, retry),
        )
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                tracing::warn!(stage = "register", result = "failed", elapsed_ms = register_started.elapsed().as_millis(), error = %e.safe_diagnostic(), "注册 VPN 节点失败");
                let message = e.to_string();
                if !previous_healthy {
                    self.shared.set_error(message.clone(), now_unix()).await;
                }
                return Err(AttemptError {
                    message,
                    retry_cluster: !e.stops_tunnel(),
                });
            }
            Err(_) => {
                let error = "注册 VPN 节点超时，请检查网络后重试".to_string();
                tracing::warn!(
                    stage = "register",
                    result = "timeout",
                    elapsed_ms = register_started.elapsed().as_millis(),
                    timeout_secs = CONNECT_TIMEOUT.as_secs(),
                    "注册 VPN 节点超时"
                );
                if !previous_healthy {
                    self.shared.set_error(error.clone(), now_unix()).await;
                }
                return Err(error.into());
            }
        };
        self.ensure_generation(generation)?;
        self.shared
            .set_state(ConnState::Connecting, now_unix())
            .await;
        tracing::info!(
            stage = "register",
            result = "succeeded",
            elapsed_ms = register_started.elapsed().as_millis(),
            routes = params.allowed_routes.len(),
            "VPN 节点注册完成"
        );

        // 注册成功后再停掉旧连接,并**等待**旧转发任务退出(它会删自己加的路由)——否则旧任务
        // 的 route delete 可能删掉下面新隧道刚加的同 CIDR 路由,导致重连黑洞。
        let cleanup_started = Instant::now();
        tracing::info!(
            stage = "previous_connection_cleanup",
            result = "started",
            "开始清理旧连接任务"
        );
        if let Err(error) = self.stop_current("重连替换旧连接").await {
            tracing::warn!(stage = "previous_connection_cleanup", result = "failed", elapsed_ms = cleanup_started.elapsed().as_millis(), error = %vpn_cli::error::redact_sensitive(&error), "旧连接任务清理失败");
            self.shared.set_error(error.clone(), now_unix()).await;
            return Err(error.into());
        }
        self.ensure_generation(generation)?;
        tracing::info!(
            stage = "previous_connection_cleanup",
            result = "succeeded",
            elapsed_ms = cleanup_started.elapsed().as_millis(),
            "旧连接任务清理完成"
        );

        // Discard any last health update from the previous gateway after its tasks exit.
        self.shared
            .set_state(ConnState::Connecting, now_unix())
            .await;
        // 2) 数据面:用户态 boringtun 隧道(本进程内开 TUN + 加路由 + 转发循环)。
        let (tx, rx) = watch::channel(false);
        // 实时路由通道(P1.4):心跳检测到 allowed_routes 变化 → 转发循环增量更新路由。
        let init_routes = daemon::RoutePolicy::from_params(&params);
        let (routes_tx, routes_rx) = watch::channel(init_routes.clone());
        let data_plane_started = Instant::now();
        tracing::info!(
            stage = "data_plane",
            result = "started",
            routes = init_routes.allowed_routes.len(),
            "开始准备 VPN 数据面"
        );
        let forward = match daemon::bring_up_tunnel(
            &self.iface,
            keypair,
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
                tracing::error!(stage = "data_plane", result = "failed", elapsed_ms = data_plane_started.elapsed().as_millis(), error = %e.safe_diagnostic(), "创建 VPN 数据面失败");
                let retry_cluster = if let (Some(gateway), true) =
                    (&params.gateway, e.is_gateway_endpoint_error())
                {
                    retry.record_failure(gateway);
                    true
                } else {
                    false
                };
                let message = e.to_string();
                self.shared.set_error(message.clone(), now_unix()).await;
                return Err(AttemptError {
                    message,
                    retry_cluster,
                });
            }
        };
        if self.ensure_generation(generation).is_err() {
            let _ = tx.send(true);
            let mut cleanup = tokio::spawn(async move {
                match forward.await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) if error.is_cleanup_failure() => {
                        Err(format!("连接取消后的数据面清理失败: {error}"))
                    }
                    // forward_loop 只在完成路由清理后才返回普通运行
                    // 错误；这种情况不应锁死之后的连接。
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error.safe_diagnostic(), "连接取消时数据面运行错误，路由已清理");
                        Ok(())
                    }
                    Err(error) => Err(format!("连接取消后的数据面任务异常: {error}")),
                }
            });
            match tokio::time::timeout(STOP_TIMEOUT, &mut cleanup).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => {
                    *self.cleanup_failure.lock().await = Some(error.clone());
                    return Err(error.into());
                }
                Ok(Err(error)) => {
                    let error = format!("连接取消后的清理监督任务异常: {error}");
                    *self.cleanup_failure.lock().await = Some(error.clone());
                    return Err(error.into());
                }
                Err(_) => {
                    // 不能 drop JoinHandle：否则 forward 会 detached，disconnect 随后会
                    // 误判清理完成。登记到 supervisor 后，stop_current 会继续等待，
                    // 若仍超时则保留句柄并拒绝 console socket 移交。
                    *self.supervisor.lock().await = Some(cleanup);
                    return Err("连接取消后的数据面清理超时，已阻止新连接"
                        .to_string()
                        .into());
                }
            }
            return Err("连接已被断开操作取消".to_string().into());
        }
        self.shared.set_vpn_ip(Some(params.vpn_ip.clone())).await;
        self.shared.set_channel_health(false, false).await;
        tracing::info!(stage = "data_plane", result = "ready", elapsed_ms = data_plane_started.elapsed().as_millis(), vpn_ip = %params.vpn_ip, iface = %self.iface, routes = init_routes.allowed_routes.len(), "VPN 数据面任务已启动（尚未确认 WireGuard 握手）");

        // 3) 管理心跳每 30s 上报；只有心跳与数据面认证握手均健康才回 Connected。
        //    数据面独立限频恢复 UDP 和握手，不因 HTTP 恢复覆盖隧道故障。只有被管理员
        //    强制下线(token 彻底失效)才返回 Err → 拆隧道 + 写错误状态,前端据此回登录页。
        let api_hb = api.clone();
        let shared = self.shared.clone();
        let hb_state = shared.clone();
        let hb_pubkey = keypair.public_key.clone();
        let connection_span = tracing::Span::current();
        let heartbeat = tokio::spawn(
            async move {
                daemon::run_heartbeat(
                    api_hb,
                    None,
                    Some(hb_pubkey),
                    rx,
                    Some(hb_state),
                    init_routes,
                    Some(routes_tx),
                )
                .await
            }
            .instrument(connection_span.clone()),
        );

        let stop_requested = Arc::new(AtomicBool::new(false));
        let supervisor_tx = tx.clone();
        let supervisor_stop_requested = stop_requested.clone();
        let supervisor = tokio::spawn(
            async move {
                daemon::supervise_connection_tasks(
                    forward,
                    heartbeat,
                    supervisor_tx,
                    supervisor_stop_requested,
                    shared,
                )
                .await
                .map_err(|error| error.to_string())
            }
            .instrument(connection_span),
        );
        let monitor = params.gateway.map(|gateway| {
            tokio::spawn(cluster::monitor(
                api,
                keypair.public_key.clone(),
                gateway,
                self.shared.clone(),
                tx.subscribe(),
                retry.clone(),
            ))
        });
        *self.shutdown.lock().await = Some(tx);
        *self.stop_requested.lock().await = Some(stop_requested);
        *self.supervisor.lock().await = Some(supervisor);

        tracing::info!(
            stage = "connect",
            result = "running",
            elapsed_ms = connection_started.elapsed().as_millis(),
            "VPN 连接后台任务已进入运行状态"
        );
        let owner = self
            .connection_owner
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        Ok(monitor.map(|task| ActiveGatewayMonitor { task, owner }))
    }

    async fn follow_gateway(
        self: Arc<Self>,
        mut generation: ConnectionGeneration,
        api: Arc<ApiClient>,
        keypair: WgKeypair,
        device: String,
        mut retry: GatewayRetry,
        mut monitor: Option<GatewayMonitor>,
    ) {
        let mut backoff =
            vpn_cli::reconnect::Backoff::new(vpn_cli::reconnect::BackoffPolicy::default());
        loop {
            if let Some(task) = monitor.take() {
                let Ok(Some(reason)) = task.await else { return };
                if let Some(gateway) = reason.failed_gateway() {
                    retry.record_failure(gateway);
                    let _operation = self.operation.lock().await;
                    if self.ensure_generation(generation).is_err() {
                        return;
                    }
                    if let Err(error) = self.stop_current("更换未通过握手的网关").await {
                        self.shared.set_error(error, now_unix()).await;
                        return;
                    }
                    self.shared
                        .set_state(ConnState::Reconnecting, now_unix())
                        .await;
                }
            } else {
                tokio::time::sleep(backoff.advance(0.5)).await;
            }
            if self.ensure_generation(generation).is_err()
                || self.cleanup_failure.lock().await.is_some()
            {
                return;
            }
            let id = self
                .attempt_seq
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            match self
                .connect_attempt(id, generation, api.clone(), &keypair, &device, &mut retry)
                .await
            {
                Ok(Some(next)) => {
                    generation.owner = Some(next.owner);
                    monitor = Some(next.task);
                    backoff.reset();
                }
                Ok(None) => return,
                Err(error) if error.retry_cluster => {
                    tracing::warn!(error = %vpn_cli::error::redact_sensitive(&error.message), "VPN 网关重连失败");
                }
                Err(_) => return,
            }
        }
    }

    /// 断开:发关停信号(隧道转发任务自行删路由/关设备,心跳停止)。
    pub async fn disconnect(&self) -> Result<(), String> {
        self.operation_epoch.fetch_add(1, Ordering::AcqRel);
        let _operation = self.operation.lock().await;
        tracing::info!("用户请求断开 VPN");
        if let Err(error) = self.stop_current("主动断开").await {
            self.shared.set_error(error.clone(), now_unix()).await;
            return Err(error);
        }
        self.shared
            .set_state(ConnState::Disconnected, now_unix())
            .await;
        Ok(())
    }

    fn ensure_connect_epoch(&self, expected_epoch: u64) -> Result<(), String> {
        if self.operation_epoch.load(Ordering::Acquire) == expected_epoch {
            Ok(())
        } else {
            Err("连接已被断开操作取消".to_string())
        }
    }

    fn ensure_generation(&self, generation: ConnectionGeneration) -> Result<(), String> {
        self.ensure_connect_epoch(generation.epoch)?;
        if generation
            .owner
            .is_some_and(|owner| owner != self.connection_owner.load(Ordering::Acquire))
        {
            return Err("VPN 网关连接已被替换".into());
        }
        Ok(())
    }

    async fn stop_current(&self, reason: &str) -> Result<(), String> {
        let started = Instant::now();
        if let Some(error) = self.cleanup_failure.lock().await.clone() {
            return Err(error);
        }
        if let Some(requested) = self.stop_requested.lock().await.take() {
            requested.store(true, Ordering::Release);
        }
        if let Some(tx) = self.shutdown.lock().await.take() {
            tracing::info!(%reason, "发送 VPN 任务关停信号");
            let _ = tx.send(true);
        }
        if let Some(mut task) = self.supervisor.lock().await.take() {
            match tokio::time::timeout(STOP_TIMEOUT, &mut task).await {
                Ok(Ok(Ok(()))) => {
                    tracing::info!(%reason, stage = "task_shutdown", result = "succeeded", elapsed_ms = started.elapsed().as_millis(), "VPN 后台任务已停止")
                }
                Ok(Ok(Err(error))) => {
                    let error = vpn_cli::error::redact_sensitive(&error);
                    *self.cleanup_failure.lock().await = Some(error.clone());
                    tracing::warn!(%reason, stage = "task_shutdown", result = "failed", elapsed_ms = started.elapsed().as_millis(), %error, "VPN 数据面清理失败");
                    return Err(error);
                }
                Ok(Err(error)) => {
                    let error = vpn_cli::error::redact_sensitive(&error.to_string());
                    *self.cleanup_failure.lock().await = Some(error.clone());
                    tracing::warn!(%reason, stage = "task_shutdown", result = "failed", elapsed_ms = started.elapsed().as_millis(), %error, "VPN supervisor panic");
                    return Err(format!("VPN 后台任务异常: {error}"));
                }
                Err(_) => {
                    tracing::warn!(%reason, stage = "task_shutdown", result = "timeout", elapsed_ms = started.elapsed().as_millis(), timeout_secs = STOP_TIMEOUT.as_secs(), "VPN 后台任务停止超时，保留旧任务并阻止重连");
                    *self.supervisor.lock().await = Some(task);
                    return Err("VPN 后台任务停止超时，为避免路由冲突已阻止重连".to_string());
                }
            }
        }
        Ok(())
    }
}

impl Default for VpnManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_ownership_depends_on_installed_connection_and_disconnect_epoch() {
        let manager = VpnManager::new();
        let epoch = manager.operation_epoch.load(Ordering::Acquire);
        let original = ConnectionGeneration {
            epoch,
            owner: Some(0),
        };
        let replacement_attempt = ConnectionGeneration { epoch, owner: None };
        assert!(manager.ensure_generation(replacement_attempt).is_ok());
        assert!(manager.ensure_generation(original).is_ok());

        manager.connection_owner.fetch_add(1, Ordering::AcqRel);
        assert!(manager.ensure_generation(original).is_err());
        assert!(manager.ensure_generation(replacement_attempt).is_ok());

        manager.operation_epoch.fetch_add(1, Ordering::AcqRel);
        assert!(manager.ensure_generation(replacement_attempt).is_err());
    }

    #[test]
    fn disconnect_epoch_invalidates_inflight_connect() {
        let manager = VpnManager::new();
        let expected = manager.operation_epoch.load(Ordering::Acquire);
        assert!(manager.ensure_connect_epoch(expected).is_ok());
        manager.operation_epoch.fetch_add(1, Ordering::AcqRel);
        assert!(manager.ensure_connect_epoch(expected).is_err());
    }
}
