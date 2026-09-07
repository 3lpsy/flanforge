use sha2::{Digest, Sha256};
use thiserror::Error;

pub const SESSION_COOKIE_NAME: &str = "flanforge_session";

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("cannot generate a session token")]
    Entropy,
}

/// A freshly minted opaque session credential: the value the cookie carries
/// and the only form the database ever sees.
#[derive(Clone, Debug)]
pub struct SessionToken {
    /// 64 hex chars from 32 random bytes; set as the cookie value.
    pub token: String,
    /// SHA-256 of the token, hex; the stored form.
    pub token_hash: String,
}

/// Mints a session token. The raw value exists only in the response cookie;
/// a database read never yields a usable credential.
///
/// # Errors
///
/// Returns an error when the system entropy source fails.
pub fn mint_session_token() -> Result<SessionToken, TokenError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| TokenError::Entropy)?;
    let token = hex(&bytes);
    let token_hash = hash_token(&token);
    Ok(SessionToken { token, token_hash })
}

/// The stored form of a presented cookie value.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// The `Set-Cookie` value for a fresh login. `HttpOnly` keeps scripts away,
/// `SameSite=Strict` is the first CSRF wall, and `Secure` is added only when
/// the request provably arrived over HTTPS — unconditional `Secure` would
/// break plain-HTTP LAN deployments, which is the admin's call, not ours.
#[must_use]
pub fn session_cookie(token: &str, max_age_seconds: u64, is_https: bool) -> String {
    let secure = if is_https { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; \
         Max-Age={max_age_seconds}{secure}"
    )
}

/// The `Set-Cookie` value that signs a browser out.
#[must_use]
pub fn clearing_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Hashes of every `flanforge_session` value in a `Cookie` header. Browsers
/// may send stale duplicates set under other paths, so every one is checked.
#[must_use]
pub fn cookie_token_hashes(cookie_header: &str) -> Vec<String> {
    cookie_header
        .split(';')
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE_NAME && !value.is_empty()).then(|| hash_token(value))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}
