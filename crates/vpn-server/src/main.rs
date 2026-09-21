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
        SqliteAccessGrantRepository, SqliteApiKeyRepository, SqliteAuditLogRepository,
        SqliteDomainEventRepository, SqliteNotificationEventRepository, SqlitePeerRepository,
        SqliteSessionRepository, SqliteSubnetRepository, SqliteSystemConfigRepository,
        SqliteUserGroupRepository, SqliteUserRepository,
    },
    services::{
        build_peer_service_with_backend, domain_event_service, ApiKeyService, Argon2Hasher,
        AuditService, AuthService, ConfigService, DomainEventService, ExternalOptionsService,
        FeishuApprovalService, FeishuAuthService, IntegrationSettingsService, JwtTokenIssuer,
        NetworkAclService, NetworkSettingsService, NotificationService, PeerService,
        ReqwestFeishuApprovalApi, ReqwestFeishuIdentityProvider, SubnetExternalOptionProvider,
        SubnetService, UserGroupExternalOptionProvider, UserGroupService, UserService,
    },
    shutdown::shutdown_or_restart,
    startup, AppState, ServerConfig,
};

fn main() -> anyhow::Result<()> {
    // 初始化 tracing（JSON 输出到 stdout，由 RUST_LOG 控制级别）
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_target(true)
        .init();

    let runtime = tokio::runtime::Runtime::new()?;
    let (restart_tx, restart_rx) = tokio::sync::watch::channel(false);
    runtime.block_on(run(restart_tx, restart_rx.clone()))?;
    // 先销毁运行时，释放监听端口、数据库连接和所有后台任务，再替换进程。
    drop(runtime);
    if *restart_rx.borrow() {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new(std::env::current_exe()?)
                .args(std::env::args_os().skip(1))
                .exec();
            return Err(error).context("重新启动服务端失败");
        }
    }
    Ok(())
}

async fn run(
    restart_tx: tokio::sync::watch::Sender<bool>,
    restart_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    // 加载配置
    let mut config = ServerConfig::from_env().context("加载配置失败")?;
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

    let peer_repo = SqlitePeerRepository::new(pool.clone());
    let config_repo = SqliteSystemConfigRepository::new(pool.clone());
    let config_service = Arc::new(ConfigService::new(config_repo.clone()));
    let integration_settings_service = Arc::new(
        IntegrationSettingsService::load_or_seed(
            config_repo.clone(),
            pool.clone(),
            &config.feishu,
            &config.feishu_approval,
            &config.feishu_approval_options,
        )
        .await
        .context("初始化集成设置失败")?,
    );
    let (feishu, feishu_approval, feishu_approval_options) =
        integration_settings_service.applied_runtime();
    config.feishu = feishu;
    config.feishu_approval = feishu_approval;
    config.feishu_approval_options = feishu_approval_options;
    let network_settings_service = Arc::new(
        NetworkSettingsService::load_or_seed(
            config_repo.clone(),
            peer_repo.clone(),
            &config.data_plane_seed,
            &config.network_settings_seed,
            config.enable_https,
            config.feishu_approval.enabled(),
        )
        .await
        .context("初始化网络参数失败")?,
    );
    let applied_network = network_settings_service.applied().clone();
    let subnet: ipnet::Ipv4Net = applied_network
        .vpn
        .vpn_subnet
        .parse()
        .with_context(|| format!("vpn_subnet 非法 CIDR：{}", applied_network.vpn.vpn_subnet))?;
    let obfs = config
        .obfs_config(&applied_network.obfs)
        .context("装配 UDP 混淆配置失败")?;
    if config.feishu_approval.enabled() && applied_network.vpn.wg_backend != "kernel" {
        anyhow::bail!("飞书审批网络授权首期仅支持 wg_backend=kernel");
    }

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
    // 用户组服务:组 CRUD + 用户分配(组的可路由网段用于访问控制)。
    let user_group_service = Arc::new(
        UserGroupService::new(
            SqliteUserGroupRepository::new(pool.clone()),
            user_repo.clone(),
        )
        .with_route_policy(subnet),
    );
    // 网段目录服务:集中维护命名网段,供各处下拉选择。
    let subnet_service = Arc::new(SubnetService::new(SqliteSubnetRepository::new(
        pool.clone(),
    )));
    let mut external_options_service =
        ExternalOptionsService::new(config.feishu_approval_options.token.clone());
    external_options_service
        .register(
            "subnets",
            Arc::new(SubnetExternalOptionProvider::new(subnet_service.clone())),
        )
        .context("注册网段外部选项数据源失败")?;
    external_options_service
        .register(
            "user-groups",
            Arc::new(UserGroupExternalOptionProvider::new(
                user_group_service.clone(),
            )),
        )
        .context("注册用户组外部选项数据源失败")?;
    // 冻结规格使用下划线；保留既有连字符地址，避免已发布的飞书表单失效。
    external_options_service
        .register(
            "user_groups",
            Arc::new(UserGroupExternalOptionProvider::new(
                user_group_service.clone(),
            )),
        )
        .context("注册用户组外部选项兼容数据源失败")?;
    let external_options_service = Arc::new(external_options_service);
    let auth_service = Arc::new(AuthService {
        user_repo,
        session_repo,
        hasher: hasher.clone(),
        issuer,
        login_attempts: LoginAttempts::new(),
    });
    let feishu_auth_service = if config.feishu.enabled() {
        Some(Arc::new(
            FeishuAuthService::new(
                config.feishu.clone(),
                Arc::new(ReqwestFeishuIdentityProvider::new(config.feishu.clone())?),
                auth_service.user_repo.clone(),
                auth_service.clone(),
            )
            .with_approval_required_for_new_accounts(config.feishu_approval.enabled()),
        ))
    } else {
        None
    };
    let api_key_service = Arc::new(ApiKeyService::new(SqliteApiKeyRepository::new(
        pool.clone(),
    )));

    // Epic 4：装配 PeerService（load-or-generate 服务端 WG 密钥 + IpPool 回填 + Noop control）
    const APPROVAL_ACL_MARKER: &str = "feishu_approval_acl_installed";
    if config.feishu_approval.enabled() {
        // WireGuard 接口恢复已有 peer 前先安装最小 drop ACL，关闭重启期间的数据面窗口。
        let bootstrap_acl =
            vpn_wireguard::NftAclController::new(&applied_network.vpn.wg_interface, subnet)?;
        bootstrap_acl.verify_available().await?;
        // 先持久化清理义务，再触碰内核状态；即使后续安装或启动失败，下次关闭功能
        // 也不会把可能残留的 final-drop 误判为“无需清理”。
        config_repo.set(APPROVAL_ACL_MARKER, "1").await?;
        bootstrap_acl
            .apply(&[], &[])
            .await
            .context("安装 nftables 启动保护规则失败（拒绝启动）")?;
    } else {
        // 标记表明本服务确实安装过 ACL；此时缺少 nft CLI 不能被当作“无需清理”。
        if config_repo.get(APPROVAL_ACL_MARKER).await?.as_deref() == Some("1") {
            let cleanup_probe =
                vpn_wireguard::NftAclController::new(&applied_network.vpn.wg_interface, subnet)?;
            cleanup_probe
                .verify_available()
                .await
                .context("审批 ACL 已安装但 nft 不可用，拒绝在未清理规则时启动")?;
        }
        vpn_wireguard::NftAclController::cleanup_owned_table_if_present()
            .await
            .context("清理已停用的审批 nftables ACL 失败")?;
        config_repo.set(APPROVAL_ACL_MARKER, "0").await?;
    }
    let peer_service = Arc::new(
        build_peer_service_with_backend(
            peer_repo,
            &config_repo,
            subnet,
            applied_network.vpn.vpn_endpoint.clone(),
            &applied_network.vpn.wg_backend,
            &applied_network.vpn.wg_interface,
            applied_network.vpn.vpn_listen_port,
            network_settings_service
                .server_routes()
                .await
                .context("加载服务端 LAN 路由失败")?,
        )
        .await
        .context("装配 PeerService 失败")?
        .with_obfs_transport(obfs.as_ref())
        .with_network_settings(network_settings_service.shared_settings())
        .with_dns_settings(network_settings_service.shared_dns_settings())
        .with_local_route_bypass(network_settings_service.shared_local_route_bypass())
        .with_registration_gate(network_settings_service.registration_gate()),
    );
    tracing::info!(
        server_public_key = %peer_service.server_public_key_string(),
        endpoint = %applied_network.vpn.vpn_endpoint,
        subnet = %applied_network.vpn.vpn_subnet,
        "服务端 WireGuard 状态已就绪"
    );
    let obfs_proxy = if let Some(obfs) = obfs.clone() {
        Some(
            vpn_server::udp_obfs::UdpObfsServer::bind(obfs, applied_network.vpn.vpn_listen_port)
                .await
                .context("初始化 UDP 混淆代理失败")?,
        )
    } else {
        None
    };
    let dns_server = vpn_server::dns_service::DnsServer::new(
        subnet,
        network_settings_service.shared_dns_settings(),
    )?;
    tracing::info!(gateway = %dns_server.gateway(), "VPN DNS 仅绑定隧道网关地址");
    let network_acl_service = if config.feishu_approval.enabled() {
        let service = Arc::new(NetworkAclService::new(
            pool.clone(),
            peer_service.clone(),
            &applied_network.vpn.wg_interface,
            subnet,
        )?);
        service
            .start()
            .await
            .context("初始化 nftables 强制 ACL 失败（拒绝启动）")?;
        service.clone().spawn();
        Some(service)
    } else {
        None
    };
    let feishu_directory_service = if config.feishu.enabled() {
        let service = Arc::new(vpn_server::services::FeishuDirectoryService::new(
            pool.clone(),
            config.feishu.app_id.clone().expect("enabled app ID"),
            config.feishu_approval.clone(),
            Arc::new(vpn_server::services::ReqwestDirectoryApi::new(
                config.feishu.clone(),
            )?),
            Some(peer_service.clone()),
            network_acl_service.clone(),
        ));
        service.clone().spawn();
        Some(service)
    } else {
        None
    };
    let notification_service = Arc::new(NotificationService::new_with_config_service(
        config.notifications.clone(),
        config_service.as_ref().clone(),
        SqliteNotificationEventRepository::new(pool.clone()),
    ));

    let feishu_approval_service = if config.feishu_approval.enabled() {
        let mut service = FeishuApprovalService::new(
            config.feishu_approval.clone(),
            SqliteAccessGrantRepository::new(pool.clone()),
            Arc::new(ReqwestFeishuApprovalApi::new(config.feishu.clone())?),
            hasher,
        )
        .with_notifications(notification_service.clone());
        if let Some(network_acl) = &network_acl_service {
            service = service.with_network_acl(network_acl.clone());
        }
        if let Some(directory) = &feishu_directory_service {
            service = service.with_directory(directory.clone());
        }
        let service = Arc::new(service);
        service.spawn_worker();
        Some(service)
    } else {
        None
    };

    // Epic 5：审计服务 + 清理任务
    let audit_repo = SqliteAuditLogRepository::new(pool.clone());
    let audit_service = Arc::new(AuditService::new(audit_repo));
    let domain_event_service = Arc::new(DomainEventService::new(SqliteDomainEventRepository::new(
        pool.clone(),
    )));

    // Story 4.6：后台离线检测任务（每 30s 扫描；panic/错误不影响主进程）
    spawn_offline_scanner(
        peer_service.clone(),
        notification_service.clone(),
        domain_event_service.clone(),
    );

    // Story 5.3：审计日志清理任务（每 24h 删除超过保留期的日志）
    spawn_audit_cleanup(audit_service.clone(), config.audit_retention_days);

    // 构造 AppState + Router
    let mut state = AppState::new()
        .with_auth_service(auth_service)
        .with_api_key_service(api_key_service)
        .with_user_service(user_service)
        .with_user_group_service(user_group_service)
        .with_subnet_service(subnet_service)
        .with_external_options_service(external_options_service)
        .with_peer_service(peer_service)
        .with_audit_service(audit_service)
        .with_config_service(config_service)
        .with_network_settings_service(network_settings_service)
        .with_integration_settings_service(integration_settings_service)
        .with_domain_event_service(domain_event_service)
        .with_notification_service(notification_service)
        .with_db_pool(pool.clone());
    if let Some(service) = network_acl_service {
        state = state.with_network_acl_service(service);
    }
    if let Some(service) = feishu_directory_service {
        state = state.with_feishu_directory_service(service);
    }
    if let Some(service) = feishu_auth_service {
        state = state.with_feishu_auth_service(service);
    }
    if let Some(service) = feishu_approval_service {
        state = state.with_feishu_approval_service(service);
    }
    if cfg!(unix) {
        state.restart_tx = Some(restart_tx);
    }
    state.trusted_proxies = std::env::var("VPN_TRUSTED_PROXIES")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse())
        .collect::<Result<Vec<std::net::IpAddr>, _>>()
        .context("VPN_TRUSTED_PROXIES 必须是逗号分隔的代理 IP")?;
    let app = build_router(state);

    // 监听端口
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("绑定地址 {} 失败", config.bind_addr))?;

    tracing::info!(addr = %config.bind_addr, "vpn-server listening");

    // 启动服务（含优雅关闭）
    if let Some(proxy) = obfs_proxy {
        tokio::select! {
            result = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown(shutdown_or_restart(restart_rx.clone())) => {
                result.context("HTTP 服务运行失败")?;
            }
            result = proxy.run() => {
                result.context("UDP 混淆代理运行失败")?;
            }
            result = dns_server.run() => {
                result.context("VPN 内置 DNS 运行失败")?;
            }
        }
    } else {
        tokio::select! {
            result = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown(shutdown_or_restart(restart_rx.clone())) => {
                result.context("HTTP 服务运行失败")?;
            }
            result = dns_server.run() => {
                result.context("VPN 内置 DNS 运行失败")?;
            }
        }
    }

    tracing::info!("vpn-server stopped");
    Ok(())
}

/// Story 4.6：每 30s 扫描一次，把心跳超时的 online peer 标记为 offline。
///
/// 任务独立运行，单次扫描出错仅记录日志不退出循环；进程退出时随 runtime 一并终止。
fn spawn_offline_scanner(
    peer_service: Arc<PeerService>,
    notification_service: Arc<NotificationService>,
    domain_event_service: Arc<DomainEventService>,
) {
    const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SCAN_INTERVAL);
        // 首个 tick 立即返回；跳过它，让首次扫描发生在一个周期后。
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let now = chrono::Utc::now().timestamp_millis();
            match peer_service.scan_offline_with_gateways(now).await {
                Ok(result) if result.marked > 0 => {
                    tracing::info!(
                        marked_offline = result.marked,
                        "离线检测：标记节点为 offline"
                    );
                    if !result.gateways.is_empty() {
                        tracing::warn!(
                            gateways = result.gateways.len(),
                            "站点网关离线，触发事件通知"
                        );
                        for gateway in &result.gateways {
                            domain_event_service
                                .publish_best_effort(
                                    domain_event_service::EVENT_GATEWAY_OFFLINE,
                                    "peer",
                                    &gateway.peer_id,
                                    gateway,
                                )
                                .await;
                        }
                        if let Err(e) = notification_service
                            .notify_gateway_offline(&result.gateways)
                            .await
                        {
                            tracing::error!(error = ?e, "站点网关离线邮件通知发送失败");
                        }
                    }
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
