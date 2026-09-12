//! 优雅关闭：SIGTERM / Ctrl+C 信号处理。

/// 等待关闭信号（SIGTERM 或 SIGINT/Ctrl+C）。
///
/// Axum 的 `axum::serve(...).with_graceful_shutdown(shutdown_signal())` 会用此函数。
/// 收到信号后服务停止接收新请求，等待 in-flight 请求完成。
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C, shutting down gracefully"),
        _ = terminate => tracing::info!("Received SIGTERM, shutting down gracefully"),
    }
}

/// 重启请求与操作系统停止信号共用 HTTP 优雅关闭路径。
pub async fn shutdown_or_restart(mut restart: tokio::sync::watch::Receiver<bool>) {
    tokio::select! {
        _ = shutdown_signal() => {},
        _ = async {
            let _ = restart.wait_for(|requested| *requested).await;
            // 让触发重启的 HTTP 响应先完成发送。
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        } => tracing::info!("Restart requested, draining HTTP connections"),
    }
}
