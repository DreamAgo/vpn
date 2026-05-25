//! JWT RS256 + Refresh Token 实现。
//!
//! 设计：
//! - Access Token = JWT RS256，15 分钟过期，含 sub(user_id) + role + exp
//! - Refresh Token = 32 字节随机数 base64 编码（不是 JWT）
//! - RSA 密钥对：启动时从文件加载，无则生成（2048 bit）

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use rsa::{
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use vpn_core::{service::TokenIssuer, AppError, Result};

/// Access Token 过期时间（15 分钟）。
pub const ACCESS_TOKEN_TTL_SECS: i64 = 15 * 60;
/// Refresh Token 过期时间（30 天）。
pub const REFRESH_TOKEN_TTL_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
pub struct AccessClaims {
    /// Subject = user_id
    pub sub: String,
    /// 用户角色（admin / user）
    pub role: String,
    /// 过期时间（unix seconds）
    pub exp: i64,
    /// 签发时间
    pub iat: i64,
}

#[derive(Debug, Error)]
pub enum TokenIssuerError {
    #[error("JWT 编码失败: {0}")]
    Encode(#[from] jsonwebtoken::errors::Error),
    #[error("密钥 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("RSA 密钥操作失败: {0}")]
    Rsa(String),
}

impl From<TokenIssuerError> for AppError {
    fn from(e: TokenIssuerError) -> Self {
        AppError::Internal(Box::new(std::io::Error::other(e.to_string())))
    }
}

pub struct JwtTokenIssuer {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl std::fmt::Debug for JwtTokenIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtTokenIssuer").finish_non_exhaustive()
    }
}

impl JwtTokenIssuer {
    /// 从文件加载 RSA 密钥；不存在则生成并保存。
    ///
    /// `data_dir` 是数据目录路径，密钥保存为 `data_dir/jwt_private.pem`。
    pub fn load_or_generate(data_dir: &Path) -> std::result::Result<Arc<Self>, TokenIssuerError> {
        std::fs::create_dir_all(data_dir)?;
        let key_path = data_dir.join("jwt_private.pem");

        let private_key = if key_path.exists() {
            let pem = std::fs::read_to_string(&key_path)?;
            RsaPrivateKey::from_pkcs8_pem(&pem).map_err(|e| TokenIssuerError::Rsa(e.to_string()))?
        } else {
            tracing::info!("生成新 RSA 2048 密钥对：{}", key_path.display());
            let mut rng = rand::rngs::OsRng;
            let key = RsaPrivateKey::new(&mut rng, 2048)
                .map_err(|e| TokenIssuerError::Rsa(e.to_string()))?;
            let pem = key
                .to_pkcs8_pem(LineEnding::LF)
                .map_err(|e| TokenIssuerError::Rsa(e.to_string()))?;
            std::fs::write(&key_path, pem.as_bytes())?;
            // 设权限 600（仅 owner 可读写）
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&key_path, perm)?;
            }
            key
        };

        let public_key = RsaPublicKey::from(&private_key);
        let priv_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| TokenIssuerError::Rsa(e.to_string()))?;
        let pub_pem = public_key
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| TokenIssuerError::Rsa(e.to_string()))?;

        let encoding_key = EncodingKey::from_rsa_pem(priv_pem.as_bytes())?;
        let decoding_key = DecodingKey::from_rsa_pem(pub_pem.as_bytes())?;

        Ok(Arc::new(Self {
            encoding_key,
            decoding_key,
        }))
    }

    /// 测试专用：从已有 RSA 私钥构造。
    #[cfg(test)]
    pub fn from_test_key(key: &RsaPrivateKey) -> Arc<Self> {
        let pub_key = RsaPublicKey::from(key);
        let priv_pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let pub_pem = pub_key.to_public_key_pem(LineEnding::LF).unwrap();
        Arc::new(Self {
            encoding_key: EncodingKey::from_rsa_pem(priv_pem.as_bytes()).unwrap(),
            decoding_key: DecodingKey::from_rsa_pem(pub_pem.as_bytes()).unwrap(),
        })
    }

    fn header() -> Header {
        Header::new(Algorithm::RS256)
    }

    fn validation() -> Validation {
        let mut v = Validation::new(Algorithm::RS256);
        v.leeway = 5;
        v.validate_exp = true;
        v
    }
}

#[async_trait]
impl TokenIssuer for JwtTokenIssuer {
    async fn issue_access(&self, user_id: &str, role: &str) -> Result<String> {
        let now = Utc::now();
        let exp = now + Duration::seconds(ACCESS_TOKEN_TTL_SECS);
        let claims = AccessClaims {
            sub: user_id.to_string(),
            role: role.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };
        jsonwebtoken::encode(&Self::header(), &claims, &self.encoding_key)
            .map_err(|e| AppError::Internal(Box::new(std::io::Error::other(e.to_string()))))
    }

    async fn issue_refresh(&self, _user_id: &str) -> Result<String> {
        // Refresh Token = 32 字节随机数 base64 编码
        // 实际的 user 关联在 sessions 表中通过 refresh_token_hash 维护
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    async fn verify_access(&self, token: &str) -> Result<(String, String)> {
        let data =
            jsonwebtoken::decode::<AccessClaims>(token, &self.decoding_key, &Self::validation())
                .map_err(|e| match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::TokenExpired,
                    _ => AppError::TokenExpired, // 任何 JWT 错误都视为 token 无效，避免泄露详细原因
                })?;
        Ok((data.claims.sub, data.claims.role))
    }

    async fn verify_refresh(&self, _token: &str) -> Result<String> {
        // 设计：Refresh Token 是不透明字符串，需要通过 sessions 表查询
        // 此 trait 方法不再适用（service 层直接走 session_repo），返回未实现错误。
        // Story 2.6 在 auth_service 中正确实现刷新流程。
        Err(AppError::Internal(Box::new(std::io::Error::other(
            "verify_refresh 应由 auth_service 通过 session_repo 实现，而非直接调用此方法",
        ))))
    }
}

/// 计算 Refresh Token 的 sha256 哈希（存数据库用）。
pub fn hash_refresh_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_issuer() -> Arc<JwtTokenIssuer> {
        let mut rng = rand::rngs::OsRng;
        // 用 1024 位密钥加速测试
        let key = RsaPrivateKey::new(&mut rng, 1024).unwrap();
        JwtTokenIssuer::from_test_key(&key)
    }

    #[tokio::test]
    async fn issue_and_verify_access_token() {
        let issuer = make_issuer();
        let token = issuer.issue_access("user-1", "admin").await.unwrap();
        let (sub, role) = issuer.verify_access(&token).await.unwrap();
        assert_eq!(sub, "user-1");
        assert_eq!(role, "admin");
    }

    #[tokio::test]
    async fn refresh_tokens_are_unique() {
        let issuer = make_issuer();
        let r1 = issuer.issue_refresh("user-1").await.unwrap();
        let r2 = issuer.issue_refresh("user-1").await.unwrap();
        assert_ne!(r1, r2);
        // base64 URL_SAFE_NO_PAD 32 字节 = 43 字符
        assert_eq!(r1.len(), 43);
    }

    #[tokio::test]
    async fn invalid_access_token_fails() {
        let issuer = make_issuer();
        let result = issuer.verify_access("invalid.jwt.token").await;
        assert!(matches!(result, Err(AppError::TokenExpired)));
    }

    #[test]
    fn refresh_token_hash_is_deterministic() {
        let h1 = hash_refresh_token("abc");
        let h2 = hash_refresh_token("abc");
        assert_eq!(h1, h2);
        let h3 = hash_refresh_token("def");
        assert_ne!(h1, h3);
    }
}
