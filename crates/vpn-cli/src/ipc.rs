//! Story 4.11: CLI <-> daemon 本地 IPC。
//!
//! 传输：Unix domain socket（unix）/ named pipe（windows，留 cfg 骨架）。
//! 协议：**按行（newline-delimited）JSON**，每行一条 [`IpcRequest`] 或
//! [`IpcResponse`]，请求-响应一一对应。
//!
//! 设计上把「消息编解码」做成纯函数（[`encode_line`] / [`decode_request`] /
//! [`decode_response`]），便于在无 socket 的环境做 round-trip 单测；真正的
//! 监听 / 连接（[`serve`] / [`send_request`]）依赖 tokio Unix socket，需运行
//! daemon 才能端到端验证。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};

/// daemon 对外暴露的连接状态机（Story 4.14）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnState {
    /// 未连接（初始 / 主动断开后）。
    Disconnected,
    /// 正在建立隧道（登录 / 注册 / 配置 IP）。
    Connecting,
    /// 已连接（隧道就绪，心跳正常）。
    Connected,
    /// 连接中断，正在按退避策略重连。
    Reconnecting,
    /// 进入错误态（不可恢复或重试耗尽）。
    Error,
}

impl ConnState {
    /// 人类可读标签（用于 `status` 打印）。
    pub fn label(&self) -> &'static str {
        match self {
            ConnState::Disconnected => "disconnected",
            ConnState::Connecting => "connecting",
            ConnState::Connected => "connected",
            ConnState::Reconnecting => "reconnecting",
            ConnState::Error => "error",
        }
    }
}

/// CLI -> daemon 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    /// 建立 VPN 连接。
    Connect,
    /// 断开 VPN 连接。
    Disconnect,
    /// 查询当前状态。
    GetStatus,
}

/// daemon -> CLI 响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcResponse {
    /// 命令已接受 / 完成。
    Ok,
    /// 当前状态快照（GetStatus / 命令后回执）。
    Status(StatusResponse),
    /// 命令失败。
    Error { message: String },
}

/// 状态快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// 连接状态。
    pub state: ConnState,
    /// 分配的 VPN IP（已连接时）。
    pub vpn_ip: Option<String>,
    /// 进入当前 Connected 状态的 unix 秒时间戳。
    pub since: Option<i64>,
    /// 累计从隧道接收字节数。
    pub bytes_rx: u64,
    /// 累计向隧道发送字节数。
    pub bytes_tx: u64,
    /// 最近一次错误描述（若有）。
    pub last_error: Option<String>,
}

impl StatusResponse {
    /// 构造一个 Disconnected 初始快照。
    pub fn disconnected() -> Self {
        Self {
            state: ConnState::Disconnected,
            vpn_ip: None,
            since: None,
            bytes_rx: 0,
            bytes_tx: 0,
            last_error: None,
        }
    }
}

// === 纯逻辑：编解码（行协议）===

/// 将一条消息编码为「JSON + 换行」的一行。
pub fn encode_line<T: Serialize>(msg: &T) -> CliResult<String> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

/// 从一行文本解码为请求。
pub fn decode_request(line: &str) -> CliResult<IpcRequest> {
    serde_json::from_str(line.trim_end()).map_err(|e| CliError::Ipc(format!("请求解码失败: {e}")))
}

/// 从一行文本解码为响应。
pub fn decode_response(line: &str) -> CliResult<IpcResponse> {
    serde_json::from_str(line.trim_end()).map_err(|e| CliError::Ipc(format!("响应解码失败: {e}")))
}

/// 默认 IPC socket 路径（unix）：`<runtime|tmp>/vpn-cli.sock`。
pub fn default_socket_path() -> PathBuf {
    // 优先 XDG_RUNTIME_DIR，否则退到临时目录。
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("vpn-cli.sock")
    } else {
        std::env::temp_dir().join("vpn-cli.sock")
    }
}

// === 传输层（需 daemon 才能端到端验证）===

#[cfg(unix)]
mod transport {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    /// 客户端：连接 daemon、发送一条请求、读取一条响应。
    pub async fn send_request(path: &std::path::Path, req: &IpcRequest) -> CliResult<IpcResponse> {
        let mut stream = UnixStream::connect(path)
            .await
            .map_err(|e| CliError::Ipc(e.to_string()))?;
        let line = encode_line(req)?;
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| CliError::Ipc(e.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|e| CliError::Ipc(e.to_string()))?;

        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        let n = reader
            .read_line(&mut resp_line)
            .await
            .map_err(|e| CliError::Ipc(e.to_string()))?;
        if n == 0 {
            return Err(CliError::Ipc("daemon 关闭了连接".to_string()));
        }
        decode_response(&resp_line)
    }

    /// 服务端：绑定 socket 并循环处理连接。`handler` 把请求映射为响应。
    ///
    /// 真机验证：实际运行于 daemon 主循环中。
    pub async fn serve<F, Fut>(path: &std::path::Path, handler: F) -> CliResult<()>
    where
        F: Fn(IpcRequest) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = IpcResponse> + Send,
    {
        // 清理可能残留的旧 socket 文件。
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).map_err(|e| CliError::Ipc(e.to_string()))?;
        loop {
            let (stream, _addr) = listener
                .accept()
                .await
                .map_err(|e| CliError::Ipc(e.to_string()))?;
            let handler = handler.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, handler).await;
            });
        }
    }

    async fn handle_conn<F, Fut>(stream: UnixStream, handler: F) -> CliResult<()>
    where
        F: Fn(IpcRequest) -> Fut,
        Fut: std::future::Future<Output = IpcResponse>,
    {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| CliError::Ipc(e.to_string()))?;
            if n == 0 {
                return Ok(()); // 对端关闭
            }
            let resp = match decode_request(&line) {
                Ok(req) => handler(req).await,
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            };
            let out = encode_line(&resp)?;
            reader
                .get_mut()
                .write_all(out.as_bytes())
                .await
                .map_err(|e| CliError::Ipc(e.to_string()))?;
            reader
                .get_mut()
                .flush()
                .await
                .map_err(|e| CliError::Ipc(e.to_string()))?;
        }
    }
}

#[cfg(not(unix))]
mod transport {
    use super::*;

    /// Windows / 其它平台：named pipe 实现留骨架。
    pub async fn send_request(
        _path: &std::path::Path,
        _req: &IpcRequest,
    ) -> CliResult<IpcResponse> {
        Err(CliError::Ipc(
            "named pipe IPC 尚未实现（仅 unix domain socket 可用）".to_string(),
        ))
    }

    /// Windows / 其它平台：named pipe server 留骨架。
    pub async fn serve<F, Fut>(_path: &std::path::Path, _handler: F) -> CliResult<()>
    where
        F: Fn(IpcRequest) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = IpcResponse> + Send,
    {
        Err(CliError::Ipc(
            "named pipe IPC server 尚未实现（仅 unix domain socket 可用）".to_string(),
        ))
    }
}

pub use transport::{send_request, serve};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        for req in [
            IpcRequest::Connect,
            IpcRequest::Disconnect,
            IpcRequest::GetStatus,
        ] {
            let line = encode_line(&req).unwrap();
            assert!(line.ends_with('\n'));
            let decoded = decode_request(&line).unwrap();
            assert_eq!(req, decoded);
        }
    }

    #[test]
    fn response_ok_roundtrip() {
        let resp = IpcResponse::Ok;
        let line = encode_line(&resp).unwrap();
        assert_eq!(decode_response(&line).unwrap(), resp);
    }

    #[test]
    fn response_status_roundtrip() {
        let resp = IpcResponse::Status(StatusResponse {
            state: ConnState::Connected,
            vpn_ip: Some("10.8.0.5".to_string()),
            since: Some(1_700_000_000),
            bytes_rx: 1234,
            bytes_tx: 5678,
            last_error: None,
        });
        let line = encode_line(&resp).unwrap();
        assert_eq!(decode_response(&line).unwrap(), resp);
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = IpcResponse::Error {
            message: "boom".to_string(),
        };
        let line = encode_line(&resp).unwrap();
        assert_eq!(decode_response(&line).unwrap(), resp);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_request("not json").is_err());
        assert!(decode_response("{").is_err());
    }

    #[test]
    fn request_uses_snake_case_tag() {
        let line = encode_line(&IpcRequest::GetStatus).unwrap();
        assert!(line.contains("\"cmd\":\"get_status\""), "got: {line}");
    }

    #[test]
    fn conn_state_serializes_snake_case() {
        let line = serde_json::to_string(&ConnState::Reconnecting).unwrap();
        assert_eq!(line, "\"reconnecting\"");
        assert_eq!(ConnState::Reconnecting.label(), "reconnecting");
    }

    #[test]
    fn status_trailing_newline_only_once() {
        let resp = IpcResponse::Ok;
        let line = encode_line(&resp).unwrap();
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn default_socket_path_has_expected_name() {
        let p = default_socket_path();
        assert!(p.ends_with("vpn-cli.sock"));
    }

    #[test]
    fn disconnected_snapshot_defaults() {
        let s = StatusResponse::disconnected();
        assert_eq!(s.state, ConnState::Disconnected);
        assert_eq!(s.bytes_rx, 0);
        assert!(s.vpn_ip.is_none());
    }
}
