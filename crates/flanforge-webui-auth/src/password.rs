use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("password hashing failed")]
    Hash,
}

/// Hashes a password into an argon2id PHC string with a fresh salt.
///
/// # Errors
///
/// Returns an error when hashing itself fails; never for a weak password —
/// policy belongs to the caller.
pub fn hash_password(password: &str) -> Result<String, CredentialError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| CredentialError::Hash)
}

/// Verifies a password against a stored PHC string. A malformed stored hash
/// reads as a mismatch, never a match.
#[must_use]
pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Verifies against the stored hash when one exists, and against a cached
/// dummy hash otherwise, so probing a username costs the same as a wrong
/// password and the result for a missing credential is always false.
#[must_use]
pub fn verify_password_or_dummy(password: &str, phc: Option<&str>) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    if let Some(phc) = phc {
        return verify_password(password, phc);
    }
    let dummy =
        DUMMY.get_or_init(|| hash_password("flanforge-timing-equalizer").unwrap_or_default());
    let _ = verify_password(password, dummy);
    false
}
