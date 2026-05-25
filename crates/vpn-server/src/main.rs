//! vpn-server 入口（仅启动调度，详细逻辑在 lib.rs）。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use sqlx::sqlite::SqlitePoolOptions;
use tracing_subscriber::EnvFilter;
use vpn_server::{
    build_router,
    ratelimit::LoginAttempts,
    repositories::{SqliteSessionRepository, SqliteUserRepository},
    services::{Argon2Hasher, AuthService, JwtTokenIssuer},
    shutdown::shutdown_signal,
    startup, AppState, ServerConfig,
};

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

    // 初始化数据库 + migrations
    std::fs::create_dir_all(&config.data_dir).context("创建数据目录失败")?;
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&config.database_url)
        .await
        .with_context(|| format!("连接数据库 {} 失败", config.database_url))?;
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("数据库 migration 失败")?;
    tracing::info!("数据库 migration 完成");

    // 初始化业务服务
    let user_repo = SqliteUserRepository::new(pool.clone());
    let session_repo = SqliteSessionRepository::new(pool);
    let hasher: Arc<dyn vpn_core::service::PasswordHasher> = Arc::new(Argon2Hasher::new());
    let issuer = JwtTokenIssuer::load_or_generate(&PathBuf::from(&config.data_dir))
        .context("加载/生成 JWT 密钥失败")?;
    let auth_service = Arc::new(AuthService {
        user_repo,
        session_repo,
        hasher,
        issuer,
        login_attempts: LoginAttempts::new(),
    });

    // 构造 AppState + Router
    let state = AppState::new().with_auth_service(auth_service);
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
