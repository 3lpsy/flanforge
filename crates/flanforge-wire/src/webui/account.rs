use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use super::user::{WEBUI_MAX_PASSWORD_BYTES, WEBUI_MIN_PASSWORD_BYTES};

/// Creates a password-backed account from the UI.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    #[validate(custom(function = "validate_username"))]
    pub username: String,
    #[validate(custom(function = "validate_password"))]
    pub password: String,
}

/// Replaces one account's password (admin action).
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct SetPasswordRequest {
    #[validate(custom(function = "validate_password"))]
    pub password: String,
}

/// One username rule for every surface: starts alphanumeric, then
/// alphanumerics plus `-`, `_`, `.`.
///
/// # Errors
///
/// Returns a validation error for anything outside that shape.
pub fn validate_username(value: &str) -> Result<(), ValidationError> {
    let is_valid = (1..=64).contains(&value.len())
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if is_valid {
        Ok(())
    } else {
        Err(ValidationError::new("username"))
    }
}

/// One password length policy for every surface.
///
/// # Errors
///
/// Returns a validation error outside the length bounds.
pub fn validate_password(value: &str) -> Result<(), ValidationError> {
    if (WEBUI_MIN_PASSWORD_BYTES..=WEBUI_MAX_PASSWORD_BYTES).contains(&value.len()) {
        Ok(())
    } else {
        Err(ValidationError::new("password"))
    }
}
