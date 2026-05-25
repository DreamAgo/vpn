//! vpn-server 入口（仅启动调度，详细逻辑在 lib.rs）。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use sqlx::sqlite::SqlitePoolOptions;
use tracing_subscriber::EnvFilter;
use vpn_server::{
    build_router,
    ratelimit::LoginAttempts,
    repositories::{
        SqliteAuditLogRepository, SqlitePeerRepository, SqliteSessionRepository,
        SqliteSystemConfigRepository, SqliteUserRepository,
    },
    services::{
        build_peer_service_with_backend, Argon2Hasher, AuditService, AuthService, JwtTokenIssuer,
        PeerService, UserService,
    },
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
    let session_repo = SqliteSessionRepository::new(pool.clone());
    let hasher: Arc<dyn vpn_core::service::PasswordHasher> = Arc::new(Argon2Hasher::new());
    let issuer = JwtTokenIssuer::load_or_generate(&PathBuf::from(&config.data_dir))
        .context("加载/生成 JWT 密钥失败")?;
    let user_service = Arc::new(UserService::new(
        user_repo.clone(),
        session_repo.clone(),
        hasher.clone(),
    ));
    let auth_service = Arc::new(AuthService {
        user_repo,
        session_repo,
        hasher,
        issuer,
        login_attempts: LoginAttempts::new(),
    });

    // Epic 4：装配 PeerService（load-or-generate 服务端 WG 密钥 + IpPool 回填 + Noop control）
    let peer_repo = SqlitePeerRepository::new(pool.clone());
    let config_repo = SqliteSystemConfigRepository::new(pool.clone());
    let subnet: ipnet::Ipv4Net = config
        .vpn_subnet
        .parse()
        .with_context(|| format!("VPN_SUBNET 非法 CIDR：{}", config.vpn_subnet))?;
    let peer_service = Arc::new(
        build_peer_service_with_backend(
            peer_repo,
            &config_repo,
            subnet,
            config.vpn_endpoint.clone(),
            &config.wg_backend,
            &config.wg_interface,
            config.vpn_listen_port,
            config.server_routes.clone(),
        )
        .await
        .context("装配 PeerService 失败")?,
    );
    tracing::info!(
        server_public_key = %peer_service.server_public_key_string(),
        endpoint = %config.vpn_endpoint,
        subnet = %config.vpn_subnet,
        "服务端 WireGuard 状态已就绪"
    );

    // Epic 5：审计服务 + 清理任务
    let audit_repo = SqliteAuditLogRepository::new(pool.clone());
    let audit_service = Arc::new(AuditService::new(audit_repo));

    // Story 4.6：后台离线检测任务（每 30s 扫描；panic/错误不影响主进程）
    spawn_offline_scanner(peer_service.clone());

    // Story 5.3：审计日志清理任务（每 24h 删除超过保留期的日志）
    spawn_audit_cleanup(audit_service.clone(), config.audit_retention_days);

    // 构造 AppState + Router
    let state = AppState::new()
        .with_auth_service(auth_service)
        .with_user_service(user_service)
        .with_peer_service(peer_service)
        .with_audit_service(audit_service);
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

/// Story 4.6：每 30s 扫描一次，把心跳超时的 online peer 标记为 offline。
///
/// 任务独立运行，单次扫描出错仅记录日志不退出循环；进程退出时随 runtime 一并终止。
fn spawn_offline_scanner(peer_service: Arc<PeerService>) {
    const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SCAN_INTERVAL);
        // 首个 tick 立即返回；跳过它，让首次扫描发生在一个周期后。
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let now = chrono::Utc::now().timestamp_millis();
            match peer_service.scan_offline(now).await {
                Ok(n) if n > 0 => {
                    tracing::info!(marked_offline = n, "离线检测：标记节点为 offline")
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = ?e, "离线检测扫描失败"),
            }
        }
    });
}

/// Story 5.3：审计日志清理任务。每 24h 执行一次，删除 created_at < now - retention_days 的日志。
///
/// 任务独立运行，单次出错仅记录日志不退出循环；进程退出时随 runtime 一并终止。
fn spawn_audit_cleanup(audit_service: Arc<AuditService>, retention_days: u32) {
    const CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    let retention_ms = retention_days as i64 * 24 * 60 * 60 * 1000;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
        loop {
            // 首个 tick 立即返回：启动后立刻清理一次旧日志，之后每 24h 一次。
            ticker.tick().await;
            let cutoff = chrono::Utc::now().timestamp_millis() - retention_ms;
            match audit_service.purge_older_than(cutoff).await {
                Ok(n) => tracing::info!(purged = n, retention_days, "审计日志清理完成"),
                Err(e) => tracing::error!(error = ?e, "审计日志清理失败"),
            }
        }
    });
}
