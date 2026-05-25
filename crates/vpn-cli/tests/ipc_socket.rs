//! Story 4.11: IPC unix socket 端到端往返测试（仅 unix）。
//!
//! 启动一个 serve 任务，客户端 send_request，验证请求-响应通路。

#![cfg(unix)]

use vpn_cli::ipc::{self, ConnState, IpcRequest, IpcResponse, StatusResponse};

#[tokio::test]
async fn unix_socket_request_response_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("test.sock");

    // 服务端 handler：GetStatus -> Status(Connected)，其余 -> Ok。
    let sock_srv = sock.clone();
    let server = tokio::spawn(async move {
        let handler = |req: IpcRequest| async move {
            match req {
                IpcRequest::GetStatus => IpcResponse::Status(StatusResponse {
                    state: ConnState::Connected,
                    vpn_ip: Some("10.8.0.5".into()),
                    since: Some(1),
                    bytes_rx: 10,
                    bytes_tx: 20,
                    last_error: None,
                }),
                _ => IpcResponse::Ok,
            }
        };
        let _ = ipc::serve(&sock_srv, handler).await;
    });

    // 等待 socket 文件出现。
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // GetStatus
    let resp = ipc::send_request(&sock, &IpcRequest::GetStatus)
        .await
        .unwrap();
    match resp {
        IpcResponse::Status(s) => {
            assert_eq!(s.state, ConnState::Connected);
            assert_eq!(s.vpn_ip, Some("10.8.0.5".into()));
        }
        other => panic!("expected Status, got {other:?}"),
    }

    // Connect -> Ok
    let resp = ipc::send_request(&sock, &IpcRequest::Connect)
        .await
        .unwrap();
    assert_eq!(resp, IpcResponse::Ok);

    server.abort();
}

#[tokio::test]
async fn send_to_missing_socket_errors() {
    let path = std::path::Path::new("/tmp/vpn-cli-nonexistent-xyz.sock");
    let _ = std::fs::remove_file(path);
    let err = ipc::send_request(path, &IpcRequest::GetStatus)
        .await
        .unwrap_err();
    assert!(matches!(err, vpn_cli::error::CliError::Ipc(_)));
}
