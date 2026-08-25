//! 业务服务实现（具体类型，由 AppState 持有）。

pub mod api_key_service;
pub mod audit_service;
pub mod auth_service;
pub mod config_service;
pub mod domain_event_service;
pub mod external_options_service;
pub mod feishu_approval_service;
pub mod feishu_auth_service;
pub mod network_acl_service;
pub mod network_settings_service;
pub mod notification_channels;
pub mod notification_service;
pub mod password_hasher;
pub mod peer_service;
pub mod subnet_service;
pub mod token_issuer;
pub mod user_group_service;
pub mod user_service;

pub use api_key_service::{ApiKeyService, VerifiedApiKey};
pub use audit_service::{infer_action, AuditService};
pub use auth_service::{AuthService, LoginOutcome};
pub use config_service::ConfigService;
pub use domain_event_service::DomainEventService;
pub use external_options_service::{
    ExternalOptionItem, ExternalOptionProvider, ExternalOptionsError,
    ExternalOptionsRegistrationError, ExternalOptionsService, SubnetExternalOptionProvider,
    UserGroupExternalOptionProvider,
};
pub use feishu_approval_service::{
    ApprovalEventHeaders, ApprovalWebhookReply, FeishuApprovalApi, FeishuApprovalService,
    ReqwestFeishuApprovalApi,
};
pub use feishu_auth_service::{FeishuAuthService, ReqwestFeishuIdentityProvider};
pub use network_acl_service::{NetworkAclService, ACL_LEASE_CAP_MS, ACL_REFRESH_INTERVAL};
pub use network_settings_service::NetworkSettingsService;
pub use notification_service::NotificationService;
pub use password_hasher::Argon2Hasher;
pub use peer_service::{
    build_peer_service, build_peer_service_with_backend, GatewayOfflineNotice, PeerConfigDownload,
    PeerService, OFFLINE_THRESHOLD_MS,
};
pub use subnet_service::SubnetService;
pub use token_issuer::{JwtTokenIssuer, TokenIssuerError};
pub use user_group_service::UserGroupService;
pub use user_service::UserService;
