//! Server-only password primitives. These are CPU-bound operations; async
//! callers must use a bounded blocking executor, not a Tokio request worker.

use argon2::password_hash::{
    rand_core::{OsRng, RngCore},
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};
use thiserror::Error;

const MEMORY_KIB: u32 = 19_456;
const ITERATIONS: u32 = 2;
const LANES: u32 = 1;
const OUTPUT_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
pub const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_HASH_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AxPasswordError {
    #[error("password must contain between 1 and 1024 bytes")]
    InvalidPasswordLength,
    #[error("stored password hash is invalid or unsupported")]
    InvalidHash,
    #[error("password salt generation failed")]
    RandomnessUnavailable,
    #[error("password hashing failed")]
    HashingFailed,
}

pub struct AxPassword;

impl AxPassword {
    /// Hashes the exact UTF-8 bytes, without trimming or normalization.
    pub fn hash(password: &str) -> Result<String, AxPasswordError> {
        validate_password(password)?;
        let mut salt_bytes = [0u8; SALT_BYTES];
        OsRng
            .try_fill_bytes(&mut salt_bytes)
            .map_err(|_| AxPasswordError::RandomnessUnavailable)?;
        let salt =
            SaltString::encode_b64(&salt_bytes).map_err(|_| AxPasswordError::HashingFailed)?;
        engine()?
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AxPasswordError::HashingFailed)
    }

    /// Wrong passwords return `false`; corrupt or unsupported stored hashes
    /// are operational errors. Neither error variant contains secret values.
    pub fn verify(password: &str, encoded_hash: &str) -> Result<bool, AxPasswordError> {
        validate_password(password)?;
        if encoded_hash.len() > MAX_HASH_BYTES {
            return Err(AxPasswordError::InvalidHash);
        }
        let hash = PasswordHash::new(encoded_hash).map_err(|_| AxPasswordError::InvalidHash)?;
        let params = Params::try_from(&hash).map_err(|_| AxPasswordError::InvalidHash)?;
        // The verifier uses PHC parameters, not the engine's defaults. Reject
        // unexpected costs before allocating memory or doing expensive work.
        let salt = hash.salt.ok_or(AxPasswordError::InvalidHash)?;
        let mut decoded_salt = [0u8; 64];
        let salt = salt
            .decode_b64(&mut decoded_salt)
            .map_err(|_| AxPasswordError::InvalidHash)?;
        if hash.algorithm.as_str() != "argon2id"
            || hash.version != Some(19)
            || params.m_cost() != MEMORY_KIB
            || params.t_cost() != ITERATIONS
            || params.p_cost() != LANES
            || hash.params.iter().count() != 3
            || salt.len() != SALT_BYTES
            || hash.hash.as_ref().map(|output| output.len()) != Some(OUTPUT_BYTES)
        {
            return Err(AxPasswordError::InvalidHash);
        }
        match engine()?.verify_password(password.as_bytes(), &hash) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(_) => Err(AxPasswordError::InvalidHash),
        }
    }
}

fn validate_password(password: &str) -> Result<(), AxPasswordError> {
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        Err(AxPasswordError::InvalidPasswordLength)
    } else {
        Ok(())
    }
}

fn engine() -> Result<Argon2<'static>, AxPasswordError> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, LANES, Some(OUTPUT_BYTES))
        .map_err(|_| AxPasswordError::HashingFailed)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_use_independent_salts_and_verify_exact_passwords() {
        let password = " a unicode password: \u{017e} ";
        let first = AxPassword::hash(password).unwrap();
        let second = AxPassword::hash(password).unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert_eq!(AxPassword::verify(password, &first), Ok(true));
        assert_eq!(AxPassword::verify(password.trim(), &first), Ok(false));
        assert_eq!(AxPassword::verify("wrong", &first), Ok(false));
    }

    #[test]
    fn rejects_empty_and_oversized_passwords_without_echoing_them() {
        for value in [String::new(), "x".repeat(MAX_PASSWORD_BYTES + 1)] {
            assert_eq!(
                AxPassword::hash(&value),
                Err(AxPasswordError::InvalidPasswordLength)
            );
            assert_eq!(
                AxPassword::verify(&value, "invalid"),
                Err(AxPasswordError::InvalidPasswordLength)
            );
        }
        assert_eq!(validate_password(&"x".repeat(MAX_PASSWORD_BYTES)), Ok(()));
        assert_eq!(
            validate_password(&"\u{017e}".repeat(MAX_PASSWORD_BYTES)),
            Err(AxPasswordError::InvalidPasswordLength)
        );
    }

    #[test]
    fn rejects_malformed_and_unbounded_hashes_before_verification() {
        let hash = AxPassword::hash("example").unwrap();
        for invalid in [
            "not-a-hash".to_string(),
            "x".repeat(MAX_HASH_BYTES + 1),
            hash.replace("argon2id", "argon2i"),
            hash.replace("v=19", "v=16"),
            hash.replace("m=19456", "m=4294967295"),
            hash.replace("t=2", "t=4294967295"),
            hash.replace("p=1", "p=2"),
            hash.replace("p=1", "p=1,data=YWJj"),
            hash.replacen(hash.split('$').nth(4).unwrap(), "YWJjZGVmZ2g", 1),
            hash.rsplit_once('$').unwrap().0.to_string(),
            format!("{}$YWJjZGVmZ2g", hash.rsplit_once('$').unwrap().0),
        ] {
            assert_eq!(
                AxPassword::verify("example", &invalid),
                Err(AxPasswordError::InvalidHash)
            );
        }
    }
}
