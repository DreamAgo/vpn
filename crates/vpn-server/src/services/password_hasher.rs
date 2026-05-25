//! Argon2id 密码哈希实现。
//!
//! 参数：OWASP 2024 推荐（m=64MB, t=3, p=2 — argon2 crate 默认即合规）。

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use vpn_core::{service::PasswordHasher as PasswordHasherTrait, AppError, Result};

#[derive(Debug, Default, Clone)]
pub struct Argon2Hasher;

impl Argon2Hasher {
    pub fn new() -> Self {
        Self
    }
}

impl PasswordHasherTrait for Argon2Hasher {
    fn hash(&self, plaintext: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(plaintext.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| {
                AppError::Internal(Box::new(std::io::Error::other(format!(
                    "argon2 hash: {}",
                    e
                ))))
            })
    }

    fn verify(&self, plaintext: &str, hash: &str) -> Result<bool> {
        let parsed = PasswordHash::new(hash).map_err(|e| {
            AppError::Internal(Box::new(std::io::Error::other(format!(
                "argon2 parse: {}",
                e
            ))))
        })?;
        Ok(Argon2::default()
            .verify_password(plaintext.as_bytes(), &parsed)
            .is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_correct_password() {
        let hasher = Argon2Hasher::new();
        let hash = hasher.hash("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(hasher
            .verify("correct horse battery staple", &hash)
            .unwrap());
    }

    #[test]
    fn verify_rejects_wrong_password() {
        let hasher = Argon2Hasher::new();
        let hash = hasher.hash("password1").unwrap();
        assert!(!hasher.verify("password2", &hash).unwrap());
    }

    #[test]
    fn each_hash_uses_different_salt() {
        let hasher = Argon2Hasher::new();
        let h1 = hasher.hash("same").unwrap();
        let h2 = hasher.hash("same").unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn verify_handles_invalid_hash() {
        let hasher = Argon2Hasher::new();
        assert!(hasher.verify("anything", "not-a-valid-hash").is_err());
    }
}
