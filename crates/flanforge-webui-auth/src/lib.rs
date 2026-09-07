//! Web UI credential mechanics: argon2id password hashing with a
//! timing-equalized verify path, and opaque session tokens whose stored form
//! is a SHA-256 hash. The OIDC relying-party flow lives in `oidc`.

mod password;
mod session;

pub mod oidc;

pub use password::{CredentialError, hash_password, verify_password, verify_password_or_dummy};
pub use session::{
    SESSION_COOKIE_NAME, SessionToken, TokenError, clearing_cookie, cookie_token_hashes,
    hash_token, mint_session_token, session_cookie,
};

#[cfg(test)]
mod tests;
