//! WireGuard 密钥对（Curve25519 / x25519）。
//!
//! 密钥以标准 WireGuard base64 文本表示（32 字节 → 44 字符）。

use base64::Engine;
use vpn_core::{AppError, Result};
use x25519_dalek::{PublicKey, StaticSecret};

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// WireGuard 密钥对（base64 文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgKeypair {
    /// 私钥（base64，32 字节）
    pub private_key: String,
    /// 公钥（base64，32 字节）
    pub public_key: String,
}

/// 生成新的 WireGuard 密钥对（x25519-dalek + OsRng）。
pub fn generate_keypair() -> WgKeypair {
    let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&secret);
    WgKeypair {
        private_key: b64().encode(secret.to_bytes()),
        public_key: b64().encode(public.as_bytes()),
    }
}

/// 从 base64 私钥推导 base64 公钥（用于校验/恢复）。
pub fn public_key_from_private(private_b64: &str) -> Result<String> {
    let bytes = b64()
        .decode(private_b64.trim())
        .map_err(|e| AppError::WireGuard(format!("私钥 base64 解码失败: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AppError::WireGuard("私钥长度必须为 32 字节".to_string()))?;
    let secret = StaticSecret::from(arr);
    let public = PublicKey::from(&secret);
    Ok(b64().encode(public.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generates_unique_keypairs() {
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let kp = generate_keypair();
            assert!(seen.insert(kp.private_key.clone()), "私钥重复");
            // base64(32 bytes) == 44 字符
            assert_eq!(kp.private_key.len(), 44);
            assert_eq!(kp.public_key.len(), 44);
        }
    }

    #[test]
    fn public_key_derivation_is_deterministic() {
        let kp = generate_keypair();
        let derived = public_key_from_private(&kp.private_key).unwrap();
        assert_eq!(derived, kp.public_key);
    }

    #[test]
    fn invalid_private_key_errors() {
        assert!(public_key_from_private("not-base64!!!").is_err());
        assert!(public_key_from_private("dG9vc2hvcnQ=").is_err()); // "tooshort"
    }
}
