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
