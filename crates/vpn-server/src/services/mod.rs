//! 业务服务实现（具体类型，由 AppState 持有）。

pub mod auth_service;
pub mod password_hasher;
pub mod peer_service;
pub mod token_issuer;
pub mod user_service;

pub use auth_service::{AuthService, LoginOutcome};
pub use password_hasher::Argon2Hasher;
pub use peer_service::{build_peer_service, PeerConfigDownload, PeerService, OFFLINE_THRESHOLD_MS};
pub use token_issuer::{JwtTokenIssuer, TokenIssuerError};
pub use user_service::UserService;
