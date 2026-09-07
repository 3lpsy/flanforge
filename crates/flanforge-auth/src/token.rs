use http::{HeaderMap, header::AUTHORIZATION};
use validator::ValidationError;

use super::error::{AuthError, TokenRejection};

/// Extracts exactly one RFC 6750 bearer credential.
///
/// # Errors
///
/// Returns an error for missing, duplicate, malformed, or non-UTF-8 headers.
pub fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or(AuthError::MissingCredentials)?;
    if values.next().is_some() {
        return Err(AuthError::InvalidToken(TokenRejection::DuplicateHeader));
    }
    let value = value
        .to_str()
        .map_err(|_| AuthError::InvalidToken(TokenRejection::NonUtf8Header))?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or(AuthError::InvalidToken(TokenRejection::Scheme))?;
    if token.len() > 16_384 {
        return Err(AuthError::InvalidToken(TokenRejection::Oversize));
    }
    validate_compact_jwt(token)
        .map_err(|_| AuthError::InvalidToken(TokenRejection::CompactShape))?;
    Ok(token)
}

fn validate_compact_jwt(value: &str) -> Result<(), ValidationError> {
    if value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(ValidationError::new("jwt"));
    }
    let is_segment = |segment: &str| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    };
    let mut segments = value.split('.');
    let header = segments.next();
    let payload = segments.next();
    let signature = segments.next();
    if header.is_some_and(is_segment)
        && payload.is_some_and(is_segment)
        && signature.is_some_and(is_segment)
        && segments.next().is_none()
    {
        Ok(())
    } else {
        Err(ValidationError::new("jwt"))
    }
}
