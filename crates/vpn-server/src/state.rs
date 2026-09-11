//! 共享应用状态（注入到所有 handler）。

use sqlx::SqlitePool;
use std::sync::Arc;
use vpn_core::time::{Clock, SystemClock};

use crate::services::{
    ApiKeyService, AuditService, AuthService, ConfigService, DomainEventService,
    ExternalOptionsService, FeishuApprovalService, FeishuAuthService, IntegrationSettingsService,
    NetworkAclService, NetworkSettingsService, NotificationService, PeerService, SubnetService,
    UserGroupService, UserService,
};

/// AppState 持有所有跨 handler 共享的资源。
///
/// 增加 service / repository 时在此结构追加字段。
#[derive(Clone)]
pub struct AppState {
    pub clock: Arc<dyn Clock>,
    pub auth_service: Option<Arc<AuthService>>,
    pub feishu_auth_service: Option<Arc<FeishuAuthService>>,
    pub feishu_approval_service: Option<Arc<FeishuApprovalService>>,
    pub external_options_service: Option<Arc<ExternalOptionsService>>,
    pub api_key_service: Option<Arc<ApiKeyService>>,
    pub user_service: Option<Arc<UserService>>,
    pub user_group_service: Option<Arc<UserGroupService>>,
    pub subnet_service: Option<Arc<SubnetService>>,
    pub peer_service: Option<Arc<PeerService>>,
    pub audit_service: Option<Arc<AuditService>>,
    pub config_service: Option<Arc<ConfigService>>,
    pub domain_event_service: Option<Arc<DomainEventService>>,
    pub notification_service: Option<Arc<NotificationService>>,
    pub network_acl_service: Option<Arc<NetworkAclService>>,
    pub network_settings_service: Option<Arc<NetworkSettingsService>>,
    pub integration_settings_service: Option<Arc<IntegrationSettingsService>>,
    pub db_pool: Option<SqlitePool>,
}

impl AppState {
    /// 用最小依赖构造 AppState（仅用于 Story 1.x 的 health handler 测试）。
    pub fn new() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            auth_service: None,
            feishu_auth_service: None,
            feishu_approval_service: None,
            external_options_service: None,
            api_key_service: None,
            user_service: None,
            user_group_service: None,
            subnet_service: None,
            peer_service: None,
            audit_service: None,
            config_service: None,
            domain_event_service: None,
            notification_service: None,
            network_acl_service: None,
            network_settings_service: None,
            integration_settings_service: None,
            db_pool: None,
        }
    }

    pub fn with_auth_service(mut self, svc: Arc<AuthService>) -> Self {
        self.auth_service = Some(svc);
        self
    }

    pub fn with_feishu_auth_service(mut self, svc: Arc<FeishuAuthService>) -> Self {
        self.feishu_auth_service = Some(svc);
        self
    }

    pub fn with_feishu_approval_service(mut self, svc: Arc<FeishuApprovalService>) -> Self {
        self.feishu_approval_service = Some(svc);
        self
    }

    pub fn with_network_acl_service(mut self, svc: Arc<NetworkAclService>) -> Self {
        self.network_acl_service = Some(svc);
        self
    }

    pub fn with_network_settings_service(mut self, svc: Arc<NetworkSettingsService>) -> Self {
        self.network_settings_service = Some(svc);
        self
    }

    pub fn with_integration_settings_service(
        mut self,
        svc: Arc<IntegrationSettingsService>,
    ) -> Self {
        self.integration_settings_service = Some(svc);
        self
    }

    /// 变更后立即重建 kernel ACL；非 kernel/测试状态下为空操作。
    pub async fn refresh_network_acl(&self) -> Result<(), vpn_core::AppError> {
        if let Some(service) = &self.network_acl_service {
            service.refresh().await?;
        }
        Ok(())
    }

    pub fn with_external_options_service(mut self, svc: Arc<ExternalOptionsService>) -> Self {
        self.external_options_service = Some(svc);
        self
    }

    pub fn with_api_key_service(mut self, svc: Arc<ApiKeyService>) -> Self {
        self.api_key_service = Some(svc);
        self
    }

    pub fn with_user_service(mut self, svc: Arc<UserService>) -> Self {
        self.user_service = Some(svc);
        self
    }

    pub fn with_user_group_service(mut self, svc: Arc<UserGroupService>) -> Self {
        self.user_group_service = Some(svc);
        self
    }

    pub fn with_subnet_service(mut self, svc: Arc<SubnetService>) -> Self {
        self.subnet_service = Some(svc);
        self
    }

    pub fn with_peer_service(mut self, svc: Arc<PeerService>) -> Self {
        self.peer_service = Some(svc);
        self
    }

    pub fn with_audit_service(mut self, svc: Arc<AuditService>) -> Self {
        self.audit_service = Some(svc);
        self
    }

    pub fn with_config_service(mut self, svc: Arc<ConfigService>) -> Self {
        self.config_service = Some(svc);
        self
    }

    pub fn with_domain_event_service(mut self, svc: Arc<DomainEventService>) -> Self {
        self.domain_event_service = Some(svc);
        self
    }

    pub fn with_notification_service(mut self, svc: Arc<NotificationService>) -> Self {
        self.notification_service = Some(svc);
        self
    }

    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// 获取 AuthService，未初始化则返回错误（启动顺序问题）。
    pub fn auth_service(&self) -> Result<Arc<AuthService>, vpn_core::AppError> {
        self.auth_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("auth_service 未初始化".to_string()))
    }

    pub fn feishu_auth_service(&self) -> Result<Arc<FeishuAuthService>, vpn_core::AppError> {
        self.feishu_auth_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("飞书登录未配置".to_string()))
    }

    pub fn integration_settings_service(
        &self,
    ) -> Result<Arc<IntegrationSettingsService>, vpn_core::AppError> {
        self.integration_settings_service.clone().ok_or_else(|| {
            vpn_core::AppError::Config("integration_settings_service 未初始化".to_string())
        })
    }

    pub fn feishu_approval_service(
        &self,
    ) -> Result<Arc<FeishuApprovalService>, vpn_core::AppError> {
        self.feishu_approval_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("飞书审批未配置".to_string()))
    }

    pub fn api_key_service(&self) -> Result<Arc<ApiKeyService>, vpn_core::AppError> {
        self.api_key_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("api_key_service 未初始化".to_string()))
    }

    /// 获取 UserService，未初始化则返回错误（启动顺序问题）。
    pub fn user_service(&self) -> Result<Arc<UserService>, vpn_core::AppError> {
        self.user_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("user_service 未初始化".to_string()))
    }

    /// 获取 UserGroupService，未初始化则返回错误（启动顺序问题）。
    pub fn user_group_service(&self) -> Result<Arc<UserGroupService>, vpn_core::AppError> {
        self.user_group_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("user_group_service 未初始化".to_string()))
    }

    /// 获取 SubnetService，未初始化则返回错误（启动顺序问题）。
    pub fn subnet_service(&self) -> Result<Arc<SubnetService>, vpn_core::AppError> {
        self.subnet_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("subnet_service 未初始化".to_string()))
    }

    /// 获取 PeerService，未初始化则返回错误（启动顺序问题）。
    pub fn peer_service(&self) -> Result<Arc<PeerService>, vpn_core::AppError> {
        self.peer_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("peer_service 未初始化".to_string()))
    }

    /// 获取 AuditService，未初始化则返回错误（启动顺序问题）。
    pub fn audit_service(&self) -> Result<Arc<AuditService>, vpn_core::AppError> {
        self.audit_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("audit_service 未初始化".to_string()))
    }

    pub fn notification_service(&self) -> Result<Arc<NotificationService>, vpn_core::AppError> {
        self.notification_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("notification_service 未初始化".to_string()))
    }

    pub fn network_settings_service(
        &self,
    ) -> Result<Arc<NetworkSettingsService>, vpn_core::AppError> {
        self.network_settings_service.clone().ok_or_else(|| {
            vpn_core::AppError::Config("network_settings_service 未初始化".to_string())
        })
    }

    pub fn config_service(&self) -> Result<Arc<ConfigService>, vpn_core::AppError> {
        self.config_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("config_service 未初始化".to_string()))
    }

    pub fn domain_event_service(&self) -> Result<Arc<DomainEventService>, vpn_core::AppError> {
        self.domain_event_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("domain_event_service 未初始化".to_string()))
    }

    pub fn db_pool(&self) -> Result<SqlitePool, vpn_core::AppError> {
        self.db_pool
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("db_pool 未初始化".to_string()))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
