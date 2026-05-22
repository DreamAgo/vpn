//! vpn-server 入口（仅启动调度，详细逻辑在 lib.rs）。

use anyhow::Context;
use tracing_subscriber::EnvFilter;
use vpn_server::{build_router, shutdown::shutdown_signal, startup, AppState, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化 tracing（JSON 输出到 stdout，由 RUST_LOG 控制级别）
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_target(true)
        .init();

    // 加载配置
    let config = ServerConfig::from_env().context("加载配置失败")?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "vpn-server starting");

    // 启动校验
    startup::validate(&config)?;

    // 构造 Router
    let state = AppState::new();
    let app = build_router(state);

    // 监听端口
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("绑定地址 {} 失败", config.bind_addr))?;

    tracing::info!(addr = %config.bind_addr, "vpn-server listening");

    // 启动服务（含优雅关闭）
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP 服务运行失败")?;

    tracing::info!("vpn-server stopped");
    Ok(())
}
