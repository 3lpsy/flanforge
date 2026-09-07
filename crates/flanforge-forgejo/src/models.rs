use flanforge_core::first_field_error;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError, ValidationErrors};

use super::client::ForgejoError;

/// Splits a response by status class so a retryable failure is never reported
/// as a semantic rejection.
pub(super) fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, ForgejoError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        tracing::warn!(
            status = status.as_u16(),
            "Forgejo returned a transient failure"
        );
        return Err(ForgejoError::Transient(status));
    }
    // A deleted registration is terminal information, not a rejection: the
    // caller can stop waiting for a resource Forgejo no longer has. It is also
    // how every ephemeral runner ends, so it is not a warning.
    if status == StatusCode::NOT_FOUND {
        tracing::debug!(status = status.as_u16(), "Forgejo has no such resource");
        return Err(ForgejoError::Absent);
    }
    tracing::warn!(status = status.as_u16(), "Forgejo rejected the request");
    Err(ForgejoError::Api)
}

pub(super) async fn bounded_json<T: serde::de::DeserializeOwned + Validate>(
    mut response: reqwest::Response,
) -> Result<T, ForgejoError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        tracing::warn!(
            response_type = std::any::type_name::<T>(),
            "Forgejo response body failed"
        );
        ForgejoError::Unavailable
    })? {
        if body.len().saturating_add(chunk.len()) > 65_536 {
            tracing::warn!(
                response_type = std::any::type_name::<T>(),
                "Forgejo response exceeded size limit"
            );
            return Err(ForgejoError::Malformed);
        }
        body.extend_from_slice(&chunk);
    }
    // An empty body carries the same meaning as an absent value, so decode it
    // as one rather than reporting malformed JSON.
    if body.is_empty() {
        body.extend_from_slice(b"null");
    }
    let value: T = serde_json::from_slice(&body).map_err(|_| {
        tracing::warn!(
            response_type = std::any::type_name::<T>(),
            "Forgejo response was not valid JSON"
        );
        ForgejoError::Malformed
    })?;
    value.validate().map_err(|errors| {
        let (field, code) = first_field_error(&errors);
        tracing::warn!(
            response_type = std::any::type_name::<T>(),
            field,
            code,
            "Forgejo response failed structural validation"
        );
        ForgejoError::Malformed
    })?;
    Ok(value)
}

#[derive(Debug, Serialize)]
pub(super) struct CreateRunner<'a> {
    pub(super) name: &'a str,
    pub(super) ephemeral: bool,
}

#[derive(Deserialize, Validate)]
pub struct RunnerCredentials {
    #[validate(range(min = 1))]
    pub id: i64,
    #[validate(custom(function = "validate_uuid"))]
    pub uuid: String,
    #[validate(custom(function = "validate_runner_token"))]
    pub token: String,
}

impl std::fmt::Debug for RunnerCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunnerCredentials")
            .field("id", &self.id)
            .field("uuid", &"[REDACTED]")
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Deserialize, Validate)]
pub(super) struct RunnerRecord {
    #[serde(default)]
    #[validate(range(min = 1))]
    pub(super) id: i64,
    #[serde(default)]
    #[validate(length(max = 255))]
    #[validate(custom(function = "validate_text"))]
    pub(super) name: String,
    #[validate(length(min = 1, max = 32))]
    #[validate(custom(function = "validate_token"))]
    pub(super) status: String,
}

#[derive(Debug, Deserialize, Validate)]
pub(super) struct ActionRunJob {
    #[validate(range(min = 1))]
    pub(super) attempt: u32,
    /// The run this job belongs to. Optional here so a Forgejo that omits it
    /// fails one named check at selection rather than every response at the
    /// JSON boundary; selection refuses to bind without it either way.
    #[serde(default)]
    pub(super) run_id: Option<i64>,
    /// Forgejo mints this as a UUID today but publishes it as opaque, so it is
    /// bounded by shape rather than by format (CORE-321).
    #[validate(custom(function = "validate_handle"))]
    pub(super) handle: String,
    #[validate(length(min = 1, max = 128))]
    #[validate(custom(function = "validate_text"))]
    pub(super) name: String,
    #[validate(custom(function = "validate_runner_labels"))]
    pub(super) runs_on: Vec<String>,
    #[validate(length(min = 1, max = 32))]
    #[validate(custom(function = "validate_token"))]
    pub(super) status: String,
}

// Forgejo serialises an empty collection as `null` rather than `[]`, so an
// absent list must read as no records instead of a malformed response.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub(super) struct ForgejoList<T>(Option<Vec<T>>);

impl<T> ForgejoList<T> {
    pub(super) fn into_inner(self) -> Vec<T> {
        self.0.unwrap_or_default()
    }
}

// Only the page shape is validated: a foreign record the daemon never acts on
// must not invalidate the whole listing. Records are validated at the point of
// use.
impl<T> Validate for ForgejoList<T> {
    fn validate(&self) -> Result<(), ValidationErrors> {
        if self.0.as_ref().is_some_and(|items| items.len() > 1_000) {
            let mut errors = ValidationErrors::new();
            errors.add("items", ValidationError::new("structure"));
            Err(errors)
        } else {
            Ok(())
        }
    }
}

fn validate_uuid(value: &str) -> Result<(), ValidationError> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| ValidationError::new("uuid"))
}

/// The bounded opaque-token shape shared by runner labels and the job handle.
pub(super) fn is_opaque_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn validate_handle(value: &str) -> Result<(), ValidationError> {
    if is_opaque_token(value) {
        Ok(())
    } else {
        Err(ValidationError::new("handle"))
    }
}

fn validate_runner_token(value: &str) -> Result<(), ValidationError> {
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ValidationError::new("runner_token"))
    }
}

fn validate_text(value: &str) -> Result<(), ValidationError> {
    if value.bytes().all(|byte| !byte.is_ascii_control()) {
        Ok(())
    } else {
        Err(ValidationError::new("text"))
    }
}

fn validate_token(value: &str) -> Result<(), ValidationError> {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(())
    } else {
        Err(ValidationError::new("token"))
    }
}

fn validate_runner_labels(values: &[String]) -> Result<(), ValidationError> {
    if values.is_empty() || values.len() > 32 || !values.iter().all(|value| is_opaque_token(value))
    {
        Err(ValidationError::new("runner_labels"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerStatus {
    Offline,
    Idle,
    Active,
}
