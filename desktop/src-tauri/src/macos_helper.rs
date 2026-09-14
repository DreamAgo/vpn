//! macOS 普通用户 GUI 与最小 root 隧道 helper 之间的安全边界。

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::{
    FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::os::unix::io::AsRawFd as _;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{watch, Mutex, Semaphore};
use vpn_cli::config::{default_device_name, CredentialRepo};
use vpn_cli::ipc::{
    decode_helper_request, decode_helper_response, encode_helper_line, HelperLogSnapshot,
    HelperRequest, HelperResponse, StatusResponse, MAX_HELPER_MESSAGE_BYTES,
};

use crate::manager::VpnManager;

const HELPER_LABEL: &str = "com.xeflow.yilian.helper";
const HELPER_PATH: &str = "/Library/PrivilegedHelperTools/com.xeflow.yilian.helper";
const PLIST_PATH: &str = "/Library/LaunchDaemons/com.xeflow.yilian.helper.plist";
const HELPER_BACKUP_PATH: &str = "/Library/PrivilegedHelperTools/com.xeflow.yilian.helper.bak";
const PLIST_BACKUP_PATH: &str = "/Library/LaunchDaemons/com.xeflow.yilian.helper.plist.bak";
const SOCKET_PATH: &str = "/var/run/com.xeflow.yilian.helper.sock";
const HELPER_LOG_DIR: &str = "/var/log/com.xeflow.yilian.helper";
const BLOCKED_TOKEN_PATH: &str = "/var/db/com.xeflow.yilian.helper.blocked-tokens";
const MAX_BLOCKED_LOGIN_HASHES: usize = 256;
const MAX_BLOCKED_TOKEN_FILE_BYTES: u64 = 32 * 1024;
const HELPER_LOG_PREFIX: &str = "helper.log";
const MAX_CONNECTIONS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const MUTATING_REQUEST_TIMEOUT: Duration = Duration::from_secs(210);
const CONNECT_GUARD_WAIT_TIMEOUT: Duration = Duration::from_secs(125);
const SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(55);
const SERVER_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(65);
// 取 8 KiB，保证最坏情况下 JSON 控制字符转义后仍小于 64 KiB IPC 上限。
const HELPER_LOG_MAX_BYTES: usize = 8 * 1024;
const HELPER_LOG_MAX_LINES: usize = 300;
const HELPER_LOG_FILE_LIMIT: u64 = 2 * 1024 * 1024;
const HELPER_LOG_ROTATIONS: usize = 3;

static INSTALL_GUARD: Mutex<()> = Mutex::const_new(());
static CONNECT_GUARD: Mutex<()> = Mutex::const_new(());
static BLOCKED_LOGIN_HASHES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static BUILD_HASH: OnceLock<Result<String, String>> = OnceLock::new();

#[derive(Debug)]
struct ConsoleSession {
    uid: u32,
    generation: u64,
}

pub fn helper_mode_requested() -> bool {
    std::env::args().any(|arg| arg == "--privileged-helper")
}

/// GUI 启动最早期固定本次运行映像的摘要，关闭“等待授权期间替换 App 路径”窗口。
pub fn initialize_gui_build_hash() -> Result<(), String> {
    current_build_hash().map(|_| ())
}

pub fn print_build_hash_requested() -> bool {
    std::env::args().any(|arg| arg == "--print-build-hash")
}

pub fn print_build_hash() -> Result<(), String> {
    println!("{}", current_build_hash()?);
    Ok(())
}

pub fn run_helper_from_args() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("特权 helper 必须由 root 运行".to_string());
    }
    // 在任何异步工作或安装替换发生前固定本进程实际启动映像的摘要。
    current_build_hash()?;
    load_blocked_login_hashes()?;
    init_helper_logging()?;
    vpn_cli::api::install_tls_crypto_provider().map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("创建 helper runtime 失败: {error}"))?;
    runtime.block_on(run_server())
}

fn init_helper_logging() -> Result<(), String> {
    std::fs::create_dir_all(HELPER_LOG_DIR)
        .map_err(|error| format!("创建 helper 日志目录失败: {error}"))?;
    std::fs::set_permissions(HELPER_LOG_DIR, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("设置 helper 日志权限失败: {error}"))?;
    let writer = std::sync::Mutex::new(BoundedLogWriter::open()?);
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "off,vpn_desktop_lib=info,vpn_cli=info,vpn_platform=info,vpn_wireguard=info",
        ))
        .with_ansi(false)
        .with_writer(writer)
        .try_init()
        .map_err(|error| format!("初始化 helper 日志失败: {error}"))
}

struct BoundedLogWriter {
    file: File,
    bytes: u64,
}

impl BoundedLogWriter {
    fn open() -> Result<Self, String> {
        let path = Path::new(HELPER_LOG_DIR).join(HELPER_LOG_PREFIX);
        let mut options = OpenOptions::new();
        options
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options
            .open(&path)
            .map_err(|error| format!("打开 helper 日志失败: {error}"))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("限制 helper 日志权限失败: {error}"))?;
        let bytes = file.metadata().map_err(|error| error.to_string())?.len();
        let mut writer = Self { file, bytes };
        if writer.bytes >= HELPER_LOG_FILE_LIMIT {
            writer.rotate().map_err(|error| error.to_string())?;
        }
        Ok(writer)
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)?;
        for index in (1..=HELPER_LOG_ROTATIONS).rev() {
            let destination = Path::new(HELPER_LOG_DIR).join(format!("{HELPER_LOG_PREFIX}.{index}"));
            let source = if index == 1 {
                Path::new(HELPER_LOG_DIR).join(HELPER_LOG_PREFIX)
            } else {
                Path::new(HELPER_LOG_DIR)
                    .join(format!("{HELPER_LOG_PREFIX}.{}", index - 1))
            };
            match std::fs::rename(source, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let path = Path::new(HELPER_LOG_DIR).join(HELPER_LOG_PREFIX);
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        self.bytes = 0;
        Ok(())
    }
}

impl std::io::Write for BoundedLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let original_len = buffer.len();
        let buffer = if buffer.len() as u64 > HELPER_LOG_FILE_LIMIT {
            &buffer[buffer.len() - HELPER_LOG_FILE_LIMIT as usize..]
        } else {
            buffer
        };
        if self.bytes.saturating_add(buffer.len() as u64) > HELPER_LOG_FILE_LIMIT {
            self.rotate()?;
        }
        std::io::Write::write_all(&mut self.file, buffer)?;
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(original_len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)
    }
}

async fn run_server() -> Result<(), String> {
    // LaunchDaemon 可能早于图形用户登录启动。无 console user 时保持 root-only，
    // 由监控任务在用户登录后原子移交 socket，避免 KeepAlive 重启风暴。
    let initial_uid = current_console_uid().unwrap_or(0);
    let socket = Path::new(SOCKET_PATH);
    if let Ok(metadata) = std::fs::symlink_metadata(socket) {
        if !metadata.file_type().is_socket() {
            return Err(format!("拒绝覆盖非 socket 路径 {SOCKET_PATH}"));
        }
        std::fs::remove_file(socket).map_err(|error| format!("清理旧 socket 失败: {error}"))?;
    }
    let listener =
        UnixListener::bind(socket).map_err(|error| format!("绑定 helper socket 失败: {error}"))?;
    secure_socket_for_uid(initial_uid)?;

    let session = Arc::new(Mutex::new(ConsoleSession {
        uid: initial_uid,
        generation: 1,
    }));
    let (generation_tx, _) = watch::channel(1_u64);
    let manager = Arc::new(VpnManager::new());
    spawn_console_user_monitor(session.clone(), generation_tx.clone(), manager.clone());
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(error = %error, "接受 IPC 连接失败，将退避重试");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tracing::warn!("helper IPC 并发达到上限，拒绝新连接");
            continue;
        };
        let manager = manager.clone();
        let session = session.clone();
        let generation_rx = generation_tx.subscribe();
        let connection_generation_tx = generation_tx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = serve_connection(
                stream,
                session,
                generation_rx,
                connection_generation_tx,
                manager,
            )
            .await
            {
                tracing::warn!(error = %vpn_cli::error::redact_sensitive(&error), "helper IPC 请求失败");
            }
        });
    }
}

fn spawn_console_user_monitor(
    session: Arc<Mutex<ConsoleSession>>,
    generation_tx: watch::Sender<u64>,
    manager: Arc<VpnManager>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let mut events = spawn_console_change_events();
        let mut events_open = true;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                event = events.recv(), if events_open => {
                    if event.is_none() {
                        events_open = false;
                    }
                }
            }
            let new_uid = current_console_uid().unwrap_or(0);
            let old_uid = session.lock().await.uid;
            if new_uid == old_uid {
                continue;
            }
            // 与请求执行共用一把锁：先使所有已认证旧连接的 generation 失效，
            // 再收回 socket、断开旧隧道，最后才允许新用户控制。
            let mut state = session.lock().await;
            if state.uid != old_uid {
                continue;
            }
            state.uid = 0;
            state.generation = state.generation.wrapping_add(1);
            generation_tx.send_replace(state.generation);
            if let Err(error) = secure_socket_for_uid(0) {
                tracing::error!(old_uid, new_uid, error = %error, "收回 helper socket 失败");
                continue;
            }
            drop(state);
            if let Err(error) = disconnect_with_timeout(&manager).await {
                tracing::error!(old_uid, new_uid, error = %vpn_cli::error::redact_sensitive(&error), "控制台用户变化时断开失败，helper 保持 root-only");
                continue;
            }
            let mut state = session.lock().await;
            if state.uid != 0 || current_console_uid().unwrap_or(0) != new_uid {
                continue;
            }
            if new_uid != 0 && secure_socket_for_uid(new_uid).is_ok() {
                state.uid = new_uid;
                tracing::info!(
                    old_uid,
                    new_uid,
                    "控制台用户变化，已断开隧道并移交 helper socket"
                );
            } else {
                tracing::warn!(old_uid, new_uid, "控制台用户不可用，helper 暂停接受请求");
            }
        }
    });
}

/// `/dev/console` 在图形会话切换时会变更 owner。用 kqueue NOTE_ATTRIB 立即唤醒，
/// 2 秒 timer 仅作为事件机制不可用时的兜底。
fn spawn_console_change_events() -> tokio::sync::mpsc::UnboundedReceiver<()> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let path = c"/dev/console";
        loop {
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                tracing::warn!(error = %std::io::Error::last_os_error(), "监听 console 用户变化失败，回退轮询");
                return;
            }
            let queue = unsafe { libc::kqueue() };
            if queue < 0 {
                let error = std::io::Error::last_os_error();
                unsafe { libc::close(fd) };
                tracing::warn!(%error, "创建 console 变化队列失败，回退轮询");
                return;
            }
            let change = libc::kevent {
                ident: fd as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_ATTRIB | libc::NOTE_DELETE | libc::NOTE_RENAME,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let registered = unsafe {
                libc::kevent(
                    queue,
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
            let received = if registered == 0 {
                unsafe {
                    libc::kevent(
                        queue,
                        std::ptr::null(),
                        0,
                        event.as_mut_ptr(),
                        1,
                        std::ptr::null(),
                    )
                }
            } else {
                -1
            };
            unsafe {
                libc::close(queue);
                libc::close(fd);
            }
            if received <= 0 || sender.send(()).is_err() {
                return;
            }
        }
    });
    receiver
}

async fn serve_connection(
    mut stream: UnixStream,
    session: Arc<Mutex<ConsoleSession>>,
    mut generation_rx: watch::Receiver<u64>,
    generation_tx: watch::Sender<u64>,
    manager: Arc<VpnManager>,
) -> Result<(), String> {
    let peer = peer_uid(&stream)?;
    let console = current_console_uid().unwrap_or(0);
    let accepted_generation = {
        let state = session.lock().await;
        if !session_is_allowed(peer, state.generation, &state, console) {
            return Err(format!("IPC 调用方 uid {peer} 不是当前控制台用户"));
        }
        state.generation
    };
    let line = tokio::time::timeout(READ_TIMEOUT, read_limited_line(&mut stream))
        .await
        .map_err(|_| "读取 helper IPC 请求超时".to_string())??;
    let request = decode_helper_request(&line).map_err(|error| error.to_string())?;
    let mut state = session.lock().await;
    let console = current_console_uid().unwrap_or(0);
    if !session_is_allowed(peer, accepted_generation, &state, console) {
        quarantine_session(&mut state, &generation_tx);
        drop(state);
        let _ = secure_socket_for_uid(0);
        let _ = disconnect_with_timeout(&manager).await;
        return Err("控制台会话已变化，拒绝旧会话请求".to_string());
    }
    drop(state);
    let response = tokio::select! {
        biased;
        changed = generation_rx.changed() => {
            let _ = changed;
            return Err("控制台会话已变化，已取消旧会话请求".to_string());
        }
        response = handle_request(request, manager.clone()) => response,
    };
    let mut state = session.lock().await;
    let console = current_console_uid().unwrap_or(0);
    if !session_is_allowed(peer, accepted_generation, &state, console) {
        quarantine_session(&mut state, &generation_tx);
        drop(state);
        let _ = secure_socket_for_uid(0);
        let _ = disconnect_with_timeout(&manager).await;
        return Err("控制台会话已变化，不再向旧会话返回数据".to_string());
    }
    drop(state);
    let output = encode_helper_line(&response).map_err(|error| error.to_string())?;
    tokio::time::timeout(CONTROL_TIMEOUT, stream.write_all(output.as_bytes()))
        .await
        .map_err(|_| "写入 helper IPC 响应超时".to_string())?
        .map_err(|error| format!("写入 helper IPC 响应失败: {error}"))?;
    Ok(())
}

async fn handle_request(request: HelperRequest, manager: Arc<VpnManager>) -> HelperResponse {
    match request {
        HelperRequest::Connect {
            server_url,
            refresh_token,
            device_name,
        } => {
            let Ok(_connect_guard) = CONNECT_GUARD.try_lock() else {
                return HelperResponse::Error {
                    message: "已有连接操作正在进行，请稍后再试".to_string(),
                };
            };
            let refresh_hash = sha256_bytes(refresh_token.as_bytes());
            {
                let Ok(blocked) = BLOCKED_LOGIN_HASHES.lock() else {
                    return HelperResponse::Error {
                        message: "注销令牌栅栏状态损坏".to_string(),
                    };
                };
                if blocked.contains(&refresh_hash) {
                    return HelperResponse::Error {
                        message: "该登录会话已注销，请重新登录后再连接".to_string(),
                    };
                }
            }
            match tokio::time::timeout(
                SERVER_CONNECT_TIMEOUT,
                manager.connect_with_credentials(server_url, refresh_token, device_name),
            )
            .await
            {
                Ok(Ok(())) => HelperResponse::Ok,
                Ok(Err(message)) => HelperResponse::Error {
                    message: bounded_message(&vpn_cli::error::redact_sensitive(&message)),
                },
                Err(_) => {
                    let _ = disconnect_with_timeout(&manager).await;
                    HelperResponse::Error {
                        message: "建立 VPN 连接超时，已清理本次连接".to_string(),
                    }
                }
            }
        }
        HelperRequest::Disconnect => match disconnect_with_timeout(&manager).await {
            Ok(()) => HelperResponse::Ok,
            Err(message) => HelperResponse::Error {
                message: bounded_message(&vpn_cli::error::redact_sensitive(&message)),
            },
        },
        HelperRequest::PrepareLogout { refresh_token } => {
            let _connect_guard = match tokio::time::timeout(
                CONNECT_GUARD_WAIT_TIMEOUT,
                CONNECT_GUARD.lock(),
            )
            .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    return HelperResponse::Error {
                        message: "等待连接事务结束超时，未提交注销".to_string(),
                    };
                }
            };
            let refresh_hash = sha256_bytes(refresh_token.as_bytes());
            if let Err(message) = record_blocked_login_hash(&refresh_hash) {
                return HelperResponse::Error {
                    message: bounded_message(&message),
                };
            }
            match disconnect_with_timeout(&manager).await {
                Ok(()) => HelperResponse::Ok,
                Err(message) => HelperResponse::Error {
                    message: bounded_message(&vpn_cli::error::redact_sensitive(&message)),
                },
            }
        }
        HelperRequest::GetStatus => {
            let mut status = manager.status().await;
            status.last_error = status
                .last_error
                .map(|message| bounded_message(&vpn_cli::error::redact_sensitive(&message)));
            HelperResponse::Status(status)
        }
        HelperRequest::GetVersion => HelperResponse::Version {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_hash: current_build_hash().unwrap_or_else(|_| "unavailable".to_string()),
        },
        HelperRequest::GetLogs => match read_helper_logs() {
            Ok(logs) => HelperResponse::Logs(logs),
            Err(message) => HelperResponse::Error {
                message: bounded_message(&vpn_cli::error::redact_sensitive(&message)),
            },
        },
    }
}

fn current_console_uid() -> Result<u32, String> {
    let uid = std::fs::metadata("/dev/console")
        .map_err(|error| format!("读取当前控制台用户失败: {error}"))?
        .uid();
    // macOS 本地交互账户从 501 起；拒绝 root、系统及隐藏服务账户。
    if uid < 501 {
        return Err("当前没有已登录的图形控制台用户".to_string());
    }
    Ok(uid)
}

fn secure_socket_for_uid(uid: u32) -> Result<(), String> {
    let path = std::ffi::CString::new(SOCKET_PATH).map_err(|error| error.to_string())?;
    if unsafe { libc::chown(path.as_ptr(), uid, !0 as libc::gid_t) } != 0 {
        return Err(format!(
            "设置 helper socket 所有者失败: {}",
            std::io::Error::last_os_error()
        ));
    }
    std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("设置 helper socket 权限失败: {error}"))
}

fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(format!(
            "读取 IPC 调用方身份失败: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(uid)
}

fn peer_is_allowed(peer: u32, active: u32, console: u32) -> bool {
    console != 0 && peer == console && peer == active
}

fn session_is_allowed(
    peer: u32,
    accepted_generation: u64,
    state: &ConsoleSession,
    console: u32,
) -> bool {
    state.generation == accepted_generation && peer_is_allowed(peer, state.uid, console)
}

fn quarantine_session(state: &mut ConsoleSession, generation_tx: &watch::Sender<u64>) {
    state.uid = 0;
    state.generation = state.generation.wrapping_add(1);
    generation_tx.send_replace(state.generation);
}

async fn read_limited_line(stream: &mut UnixStream) -> Result<String, String> {
    let mut data = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("读取 helper IPC 失败: {error}"))?;
        if n == 0 {
            return Err("helper IPC 请求未完整结束".to_string());
        }
        let end = chunk[..n]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(n);
        if data.len() + end > MAX_HELPER_MESSAGE_BYTES {
            return Err("helper IPC 消息超过长度限制".to_string());
        }
        data.extend_from_slice(&chunk[..end]);
        if data.last() == Some(&b'\n') {
            return String::from_utf8(data).map_err(|_| "helper IPC 消息不是 UTF-8".to_string());
        }
    }
}

async fn request(request: &HelperRequest) -> Result<HelperResponse, String> {
    let timeout = match request {
        HelperRequest::Connect { .. }
        | HelperRequest::Disconnect
        | HelperRequest::PrepareLogout { .. } => MUTATING_REQUEST_TIMEOUT,
        _ => CONTROL_TIMEOUT,
    };
    tokio::time::timeout(timeout, async {
        let mut stream = UnixStream::connect(SOCKET_PATH)
            .await
            .map_err(|error| format!("无法连接特权 helper: {error}"))?;
        let line = encode_helper_line(request).map_err(|error| error.to_string())?;
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("发送 helper 请求失败: {error}"))?;
        let response = read_limited_line(&mut stream).await?;
        decode_helper_response(&response).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "特权 helper 请求超时".to_string())?
}

async fn ensure_ready() -> Result<(), String> {
    let _guard = INSTALL_GUARD.lock().await;
    let process_lock = tokio::task::spawn_blocking(InstallLock::acquire)
        .await
        .map_err(|error| format!("等待 helper 安装锁失败: {error}"))??;
    let expected = tokio::task::spawn_blocking(current_build_hash)
        .await
        .map_err(|error| format!("计算客户端摘要失败: {error}"))??;

    let installed = tokio::task::spawn_blocking(|| file_sha256(Path::new(HELPER_PATH)))
        .await
        .map_err(|error| error.to_string())??;
    if installed.as_deref() == Some(expected.as_str()) {
        if helper_identity_matches(&expected).await {
            return Ok(());
        }
        let kickstart_error = tokio::task::spawn_blocking(try_kickstart)
            .await
            .map_err(|error| error.to_string())?
            .err();
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if helper_identity_matches(&expected).await {
                return Ok(());
            }
        }
        // 只有 root 事务遗留的 .bak 能证明上次安装在 bootstrap 前中断；此时允许
        // 一次授权修复。普通“匹配但暂时不可达”仍禁止重装，避免日常重复密码框。
        if backup_artifacts_exist()
            && std::env::var_os("VPN_DESKTOP_NO_HELPER_INSTALL").is_none()
        {
            let repair_expected = expected.clone();
            tokio::task::spawn_blocking(move || install_helper(&repair_expected))
                .await
                .map_err(|error| format!("helper 恢复任务失败: {error}"))??;
            for _ in 0..30 {
                if helper_identity_matches(&expected).await {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        drop(process_lock);
        return Err(match kickstart_error {
            Some(error) => format!(
                "已安装的特权 helper 与当前版本一致但不可达；launchd 恢复失败: {}",
                bounded_message(&error)
            ),
            None => "已安装的特权 helper 与当前版本一致但仍不可达，请查看 helper 日志".to_string(),
        });
    }
    if std::env::var_os("VPN_DESKTOP_NO_HELPER_INSTALL").is_some() {
        return Err("特权 helper 缺失或版本不同，且已禁用自动安装".to_string());
    }
    if installed.is_some() {
        match request(&HelperRequest::Disconnect).await {
            Ok(response) => response_result(response)
                .map_err(|error| format!("旧 helper 无法安全断开，已取消升级: {error}"))?,
            Err(error) => tracing::warn!(
                error = %vpn_cli::error::redact_sensitive(&error),
                "旧 helper 不可达，将由授权安装事务 bootout 后替换"
            ),
        }
    }
    tokio::task::spawn_blocking(move || install_helper(&expected))
        .await
        .map_err(|error| format!("helper 安装任务失败: {error}"))??;
    drop(process_lock);
    for _ in 0..30 {
        if helper_identity_matches(&current_build_hash()?).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("特权 helper 已安装但未能启动，请查看系统日志".to_string())
}

fn backup_artifacts_exist() -> bool {
    Path::new(HELPER_BACKUP_PATH).exists() || Path::new(PLIST_BACKUP_PATH).exists()
}

async fn helper_identity_matches(expected_hash: &str) -> bool {
    matches!(
        request(&HelperRequest::GetVersion).await,
        Ok(HelperResponse::Version { build_hash, .. }) if build_hash == expected_hash
    )
}

pub async fn connect() -> Result<(), String> {
    let repo = CredentialRepo::file().map_err(|error| error.to_string())?;
    let server_url = repo
        .server_url()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "未登录:请先登录".to_string())?;
    let refresh_token = repo
        .refresh_token()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "未登录:请先登录".to_string())?;
    ensure_ready().await?;
    response_result(
        request(&HelperRequest::Connect {
            server_url,
            refresh_token,
            device_name: default_device_name(),
        })
        .await?,
    )
}

pub async fn disconnect() -> Result<(), String> {
    response_result(request(&HelperRequest::Disconnect).await?)
}

/// 退出只断开隧道，保留登录凭证；未运行 helper 时无需安装或提权。
pub async fn disconnect_before_exit() -> Result<(), String> {
    match request(&HelperRequest::Disconnect).await {
        Ok(response) => response_result(response),
        Err(error) => {
            let running = tokio::task::spawn_blocking(helper_job_is_running)
                .await
                .map_err(|join_error| format!("检查 helper 状态失败: {join_error}"))?;
            if running {
                Err(format!("无法确认 VPN 已断开: {error}"))
            } else {
                Ok(())
            }
        }
    }
}

pub async fn disconnect_before_logout() -> Result<(), String> {
    if !Path::new(HELPER_PATH).exists() && !Path::new(SOCKET_PATH).exists() {
        return Ok(());
    }
    let repo = CredentialRepo::file().map_err(|error| error.to_string())?;
    let refresh_token = repo
        .refresh_token()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    match request(&HelperRequest::PrepareLogout { refresh_token }).await {
        Ok(response) => response_result(response),
        Err(error) => {
            let running = tokio::task::spawn_blocking(helper_job_is_running)
                .await
                .map_err(|join_error| format!("检查 helper 状态失败: {join_error}"))?;
            if running {
                Err(format!("无法确认 VPN 已断开，已保留登录状态: {error}"))
            } else {
                Ok(())
            }
        }
    }
}

fn response_result(response: HelperResponse) -> Result<(), String> {
    match response {
        HelperResponse::Ok => Ok(()),
        HelperResponse::Error { message } => Err(message),
        _ => Err("特权 helper 返回了意外响应".to_string()),
    }
}

fn bounded_message(message: &str) -> String {
    const MAX_CHARS: usize = 4096;
    let mut chars = message.chars();
    let value: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{value}…")
    } else {
        value
    }
}

async fn disconnect_with_timeout(manager: &VpnManager) -> Result<(), String> {
    tokio::time::timeout(SERVER_DISCONNECT_TIMEOUT, manager.disconnect())
        .await
        .map_err(|_| "断开 VPN 超时，helper 已保持禁止新连接状态".to_string())?
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn load_blocked_login_hashes() -> Result<(), String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = match options.open(BLOCKED_TOKEN_PATH) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("读取注销令牌栅栏失败: {error}")),
    };
    let content = if let Some(file) = file.as_mut() {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.nlink() != 1 {
            return Err("注销令牌栅栏文件身份异常".to_string());
        }
        if metadata.len() > MAX_BLOCKED_TOKEN_FILE_BYTES {
            return Err("注销令牌栅栏文件超过安全上限".to_string());
        }
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|error| format!("读取注销令牌栅栏失败: {error}"))?;
        content
    } else {
        String::new()
    };
    let hashes = content
        .lines()
        .filter(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if hashes.len() > MAX_BLOCKED_LOGIN_HASHES {
        return Err("注销令牌栅栏条目超过安全上限".to_string());
    }
    *BLOCKED_LOGIN_HASHES
        .lock()
        .map_err(|_| "注销令牌栅栏状态损坏".to_string())? = hashes;
    Ok(())
}

fn record_blocked_login_hash(hash: &str) -> Result<(), String> {
    let mut blocked = BLOCKED_LOGIN_HASHES
        .lock()
        .map_err(|_| "注销令牌栅栏状态损坏".to_string())?;
    let mut updated = blocked.clone();
    if !updated.iter().any(|item| item == hash) {
        if updated.len() >= MAX_BLOCKED_LOGIN_HASHES {
            return Err("注销令牌安全栅栏已满；为避免旧凭据复活，已拒绝注销，请联系管理员清理已吊销条目".to_string());
        }
        updated.push(hash.to_string());
    }

    // 即使内存中已有该 hash，也重写并 fsync：上一次 rename 成功但
    // 目录 fsync 失败时，后续重试必须能重新确认持久性。

    let temp_path = format!("{BLOCKED_TOKEN_PATH}.new.{}", std::process::id());
    let _ = std::fs::remove_file(&temp_path);
    let mut options = OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&temp_path)
        .map_err(|error| format!("写入注销令牌栅栏失败: {error}"))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("限制注销令牌栅栏权限失败: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.nlink() != 1 {
        return Err("注销令牌栅栏文件身份异常".to_string());
    }
    let content = updated.join("\n") + "\n";
    std::io::Write::write_all(&mut file, content.as_bytes())
        .map_err(|error| format!("写入注销令牌栅栏失败: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("同步注销令牌栅栏失败: {error}"))?;
    std::fs::rename(&temp_path, BLOCKED_TOKEN_PATH)
        .map_err(|error| format!("提交注销令牌栅栏失败: {error}"))?;
    // rename 已在当前文件系统提交后立即 fail-closed 更新内存；即使随后目录
    // fsync 报错，本进程也绝不能重新接受这个 token。
    *blocked = updated;
    File::open("/var/db")
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("同步注销令牌栅栏目录失败: {error}"))?;
    Ok(())
}

pub async fn status() -> StatusResponse {
    match request(&HelperRequest::GetStatus).await {
        Ok(HelperResponse::Status(status)) => status,
        Ok(HelperResponse::Error { message }) | Err(message) => {
            let mut status = StatusResponse::disconnected();
            if Path::new(HELPER_PATH).exists() {
                status.last_error = Some(vpn_cli::error::redact_sensitive(&message));
            }
            status
        }
        _ => StatusResponse::disconnected(),
    }
}

pub async fn logs() -> Result<HelperLogSnapshot, String> {
    match request(&HelperRequest::GetLogs).await? {
        HelperResponse::Logs(logs) => Ok(logs),
        HelperResponse::Error { message } => Err(message),
        _ => Err("特权 helper 返回了意外日志响应".to_string()),
    }
}

struct InstallLock(File);

impl InstallLock {
    fn acquire() -> Result<Self, String> {
        let uid = unsafe { libc::getuid() };
        let path = std::env::temp_dir().join(format!("{HELPER_LABEL}-{uid}.install.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|error| format!("打开 helper 安装锁失败: {error}"))?;
        if file.metadata().map_err(|error| error.to_string())?.uid() != uid {
            return Err("helper 安装锁所有者异常".to_string());
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if std::time::Instant::now() >= deadline {
                return Err(format!("另一个客户端正在安装 helper，请稍后重试: {error}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Self(file))
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn current_build_hash() -> Result<String, String> {
    BUILD_HASH.get_or_init(compute_current_build_hash).clone()
}

fn compute_current_build_hash() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| format!("无法定位当前程序: {error}"))?;
    file_sha256(&exe)?.ok_or_else(|| "当前程序不存在".to_string())
}

fn file_sha256(path: &Path) -> Result<Option<String>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取文件摘要失败: {error}")),
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|error| format!("读取文件摘要失败: {error}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}

fn try_kickstart() -> Result<(), String> {
    let output = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", &format!("system/{HELPER_LABEL}")])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn helper_job_is_running() -> bool {
    match std::process::Command::new("/bin/launchctl")
        .args(["print", &format!("system/{HELPER_LABEL}")])
        .output()
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).contains("state = running")
        }
        Ok(output) => !String::from_utf8_lossy(&output.stderr).contains("Could not find service"),
        Err(_) => true,
    }
}

fn install_helper(expected_hash: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("无法定位客户端程序: {error}"))?;
    let command = install_shell_command(&exe, expected_hash);
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        apple_script_escape(&command)
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|error| format!("无法请求管理员授权: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(
        if stderr.contains("User canceled") || stderr.contains("-128") {
            "已取消安装特权 helper；需要完成一次管理员授权才能连接".to_string()
        } else {
            format!("安装特权 helper 失败: {}", stderr.trim())
        },
    )
}

fn install_shell_command(exe: &Path, expected_hash: &str) -> String {
    // plist 内容是编译进程序的固定数据，不接受路径、label 或参数输入。
    let plist_b64 = base64::engine::general_purpose::STANDARD.encode(render_plist());
    format!(
        "set -e; H={}; P={}; L=/var/run/{HELPER_LABEL}.install.lock; /usr/bin/touch $L; \
         /usr/sbin/chown root:wheel $L; /bin/chmod 600 $L; exec 9>$L; /usr/bin/lockf -t 30 9; \
         N=$H.new.$$; Q=$P.new.$$; HB=$H.bak; PB=$P.bak; ARMED=0; \
         if [ -e $HB ] || [ -e $PB ]; then CUR_OK=0; \
         if [ -x $H ] && [ -f $P ] && /usr/bin/plutil -lint $P >/dev/null 2>&1; then CUR_OK=1; fi; \
         if [ $CUR_OK -eq 0 ]; then /bin/launchctl bootout system/{HELPER_LABEL} >/dev/null 2>&1 || true; \
         [ ! -e $HB ] || /bin/mv -f $HB $H; [ ! -e $PB ] || /bin/mv -f $PB $P; \
         [ ! -e $P ] || /bin/launchctl bootstrap system $P >/dev/null 2>&1 || true; fi; fi; \
         rollback() {{ R=$?; trap - EXIT HUP INT TERM; /bin/rm -f $N $Q; if [ $ARMED -eq 1 ]; then \
         /bin/rm -f $H $P; [ ! -e $HB ] || /bin/mv -f $HB $H; [ ! -e $PB ] || /bin/mv -f $PB $P; \
         [ ! -e $P ] || /bin/launchctl bootstrap system $P >/dev/null 2>&1 || true; fi; exit $R; }}; \
         trap rollback EXIT HUP INT TERM; /usr/bin/install -o root -g wheel -m 755 {} $N; \
         ACT=$(/usr/bin/shasum -a 256 $N | /usr/bin/awk '{{print $1}}'); [ \"$ACT\" = {} ]; \
         /bin/echo {} | /usr/bin/base64 -D > $Q; /usr/sbin/chown root:wheel $Q; /bin/chmod 644 $Q; \
         /usr/bin/plutil -lint $Q >/dev/null; /bin/rm -f $HB $PB; \
         [ ! -e $H ] || /bin/cp -p $H $HB; [ ! -e $P ] || /bin/cp -p $P $PB; /bin/sync; ARMED=1; \
         /bin/launchctl bootout system/{HELPER_LABEL} >/dev/null 2>&1 || true; \
         /bin/mv -f $N $H; /bin/mv -f $Q $P; /bin/sync; /bin/launchctl bootstrap system $P; \
         ARMED=0; /bin/rm -f $HB $PB; /bin/sync; trap - EXIT HUP INT TERM",
        shell_quote(HELPER_PATH), shell_quote(PLIST_PATH), shell_quote(&exe.to_string_lossy()),
        shell_quote(expected_hash), shell_quote(&plist_b64)
    )
}

fn render_plist() -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{HELPER_LABEL}</string><key>ProgramArguments</key><array><string>{HELPER_PATH}</string><string>--privileged-helper</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ProcessType</key><string>Background</string></dict></plist>\n")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn apple_script_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn read_helper_logs() -> Result<HelperLogSnapshot, String> {
    let mut files = std::fs::read_dir(HELPER_LOG_DIR)
        .map_err(|error| format!("读取 helper 日志目录失败: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(HELPER_LOG_PREFIX)
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.metadata().and_then(|metadata| metadata.modified()).ok());
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut prefix_is_partial = false;
    for entry in files.into_iter().rev() {
        if bytes.len() >= HELPER_LOG_MAX_BYTES {
            truncated = true;
            break;
        }
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1 {
            continue;
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut file = options
            .open(&path)
            .map_err(|error| format!("打开 helper 日志失败: {error}"))?;
        let length = file.metadata().map_err(|error| error.to_string())?.len();
        let take = (HELPER_LOG_MAX_BYTES - bytes.len()).min(length as usize);
        file.seek(SeekFrom::Start(length.saturating_sub(take as u64)))
            .map_err(|error| error.to_string())?;
        let mut part = Vec::with_capacity(take);
        file.take(take as u64)
            .read_to_end(&mut part)
            .map_err(|error| error.to_string())?;
        if !bytes.is_empty() && !part.ends_with(b"\n") {
            part.push(b'\n');
        }
        part.extend_from_slice(&bytes);
        bytes = part;
        if take < length as usize {
            truncated = true;
            prefix_is_partial = true;
        }
    }
    let decoded = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = decoded.lines().collect();
    let incomplete_prefix = usize::from(prefix_is_partial).min(lines.len());
    let first = incomplete_prefix.max(lines.len().saturating_sub(HELPER_LOG_MAX_LINES));
    truncated |= first > 0;
    let safe = redact_log_lines(&lines[first..]);
    Ok(HelperLogSnapshot {
        content: safe.join("\n"),
        line_count: safe.len(),
        truncated,
    })
}

fn redact_log_lines(lines: &[&str]) -> Vec<String> {
    let mut redact_block = false;
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() || looks_like_log_record(line) {
                redact_block = false;
            }
            let redacted = vpn_cli::error::redact_sensitive(line);
            let sensitive = redacted != **line;
            let output = if redact_block || sensitive {
                "[REDACTED sensitive diagnostic]".to_string()
            } else {
                redacted
            };
            if sensitive {
                redact_block = true;
            }
            output
        })
        .collect()
}

fn looks_like_log_record(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 11
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && (bytes[10] == b'T' || bytes[10] == b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_requires_active_console_user() {
        assert!(peer_is_allowed(501, 501, 501));
        assert!(!peer_is_allowed(502, 501, 501));
        assert!(!peer_is_allowed(501, 501, 502));
        assert!(!peer_is_allowed(0, 0, 0));
        let state = ConsoleSession {
            uid: 501,
            generation: 8,
        };
        assert!(session_is_allowed(501, 8, &state, 501));
        assert!(!session_is_allowed(501, 7, &state, 501));
    }

    #[test]
    fn privileged_install_is_hash_pinned_and_transactional() {
        let command = install_shell_command(Path::new("/tmp/App bin"), &"a".repeat(64));
        assert!(command.contains("shasum -a 256"));
        assert!(command.contains("trap rollback EXIT HUP INT TERM"));
        assert!(command.contains("base64 -D"));
        assert!(command.contains("/var/run/com.xeflow.yilian.helper.install.lock"));
        assert!(command.contains("/usr/bin/lockf -t 30 9"));
        assert!(!command.contains("sudoers"));
        assert!(command.contains(
            "[ ! -e $H ] || /bin/cp -p $H $HB; [ ! -e $P ] || /bin/cp -p $P $PB; /bin/sync; ARMED=1; /bin/launchctl bootout"
        ));
        assert!(command.contains("if [ -e $HB ] || [ -e $PB ]; then CUR_OK=0"));
        assert!(std::process::Command::new("/bin/sh")
            .args(["-n", "-c", &command])
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn macos_lockf_accepts_an_open_file_descriptor() {
        let path = std::env::temp_dir().join(format!(
            "{HELPER_LABEL}-lockf-test-{}",
            std::process::id()
        ));
        let command = format!(
            "set -e; L={}; /usr/bin/touch $L; exec 9>$L; /usr/bin/lockf -t 1 9",
            shell_quote(&path.to_string_lossy())
        );
        assert!(std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .status()
            .unwrap()
            .success());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plist_has_only_fixed_helper_arguments() {
        let plist = render_plist();
        assert!(plist.contains(HELPER_PATH));
        assert!(plist.contains("--privileged-helper"));
        assert!(!plist.contains("allowed-uid"));
    }

    #[test]
    fn helper_log_redaction_covers_sensitive_continuation() {
        let safe = redact_log_lines(&[
            "Authorization:",
            "raw-value",
            "another-secret-line",
            "2026-08-25T12:00:00Z INFO ready",
        ]);
        assert_eq!(
            safe,
            [
                "[REDACTED sensitive diagnostic]",
                "[REDACTED sensitive diagnostic]",
                "[REDACTED sensitive diagnostic]",
                "2026-08-25T12:00:00Z INFO ready"
            ]
        );
    }
}
