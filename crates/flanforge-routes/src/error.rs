use std::borrow::Cow;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use flanforge_core::{Allocation, AllocationId, AllocationState};
use serde::Serialize;

use flanforge_auth::AuthError;
use flanforge_manager::{ManagerError, WorkerError};

#[derive(Debug)]
pub enum ApiError {
    Authentication,
    IdentityProvider,
    ServiceUnavailable,
    BadRequest,
    Forbidden,
    NotFound,
    /// A named machine no hot record claims. Distinct from `NotFound` so the
    /// body names the right noun: the operator asked about a guest, not an
    /// allocation.
    UnknownHotGuest,
    /// A named web UI account that does not exist.
    UnknownUser,
    /// A refusal whose message is safe to show an operator.
    Conflict(&'static str),
    Busy,
    Worker(WorkerFailure),
    Timeout,
    Internal,
}

/// What an authenticated caller is told about a failed allocation. The reason
/// is the worker's message already logged at warn level — never config
/// internals or credentials.
#[derive(Debug, Default)]
pub struct WorkerFailure {
    pub allocation_id: Option<AllocationId>,
    pub state: Option<AllocationState>,
    pub reason: Option<String>,
}

impl WorkerFailure {
    #[must_use]
    pub fn from_allocation(allocation: &Allocation) -> Self {
        Self {
            allocation_id: Some(allocation.id),
            state: Some(allocation.state),
            reason: allocation.error.clone(),
        }
    }
}

impl From<&WorkerError> for WorkerFailure {
    fn from(error: &WorkerError) -> Self {
        Self {
            reason: Some(error.to_string()),
            ..Self::default()
        }
    }
}

/// The one place an authentication refusal is logged. The reason is named at a
/// level the daemon runs; the response body is unchanged and learns nothing.
impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::MissingCredentials => tracing::warn!(
                reason = "missing_credentials",
                "allocation authentication rejected"
            ),
            AuthError::InvalidToken(rejection) => tracing::warn!(
                reason = rejection.code(),
                field = rejection.field(),
                detail = rejection.detail(),
                "allocation authentication rejected"
            ),
            AuthError::Unavailable(failure) => {
                tracing::warn!(reason = failure.code(), "OIDC provider is unavailable");
            }
        }
        match error {
            AuthError::Unavailable(_) => Self::IdentityProvider,
            AuthError::MissingCredentials | AuthError::InvalidToken(_) => Self::Authentication,
        }
    }
}

impl From<ManagerError> for ApiError {
    fn from(error: ManagerError) -> Self {
        match &error {
            ManagerError::DuplicateAllocation(_)
            | ManagerError::Transition(_)
            | ManagerError::HotTransition(_)
            | ManagerError::Store(_) => tracing::error!(%error, "allocation manager failed"),
            ManagerError::Worker(_) => tracing::warn!(%error, "allocation worker failed"),
            ManagerError::Authorization(_) => {
                tracing::warn!(reason = %error, "allocation authorization rejected");
            }
            // Refusals an operator has to be able to see; `Busy` is already
            // logged with its reason and holder where admission decides it.
            _ => tracing::info!(reason = %error, "allocation request rejected"),
        }
        match error {
            ManagerError::Authorization(_) | ManagerError::RequestExceedsProfile => Self::Forbidden,
            ManagerError::NotFound(_) => Self::NotFound,
            ManagerError::UnknownHotGuest(_) => Self::UnknownHotGuest,
            ManagerError::Busy { .. } => Self::Busy,
            ManagerError::ShuttingDown => Self::ServiceUnavailable,
            ManagerError::UnknownProfile(_)
            | ManagerError::InvalidVmName(_)
            | ManagerError::InvalidRunnerLabel(_)
            | ManagerError::InvalidRequest
            | ManagerError::InvalidRunnerId => Self::BadRequest,
            ManagerError::Worker(worker) => Self::Worker(WorkerFailure::from(&worker)),
            ManagerError::DuplicateAllocation(_)
            | ManagerError::Transition(_)
            | ManagerError::HotTransition(_)
            | ManagerError::Store(_) => Self::Internal,
        }
    }
}

impl From<WorkerError> for ApiError {
    fn from(error: WorkerError) -> Self {
        tracing::warn!(%error, "allocation worker failed");
        Self::Worker(WorkerFailure::from(&error))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Authentication => (
                StatusCode::UNAUTHORIZED,
                ErrorBody::new("authentication required"),
            ),
            Self::IdentityProvider => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorBody::new("identity provider unavailable"),
            ),
            Self::ServiceUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorBody::new("service is shutting down"),
            ),
            Self::BadRequest => (StatusCode::BAD_REQUEST, ErrorBody::new("invalid request")),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                ErrorBody::new("request is not authorized"),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                ErrorBody::new("allocation not found"),
            ),
            Self::UnknownHotGuest => (StatusCode::NOT_FOUND, ErrorBody::new("hot guest not found")),
            Self::UnknownUser => (StatusCode::NOT_FOUND, ErrorBody::new("user not found")),
            Self::Conflict(message) => (StatusCode::CONFLICT, ErrorBody::new(message)),
            Self::Busy => (StatusCode::CONFLICT, ErrorBody::new("FlanForge is busy")),
            // The caller is authenticated on this path; the reason is the
            // worker message already logged at warn level, so it may be shown.
            Self::Worker(failure) => (StatusCode::BAD_GATEWAY, ErrorBody::from(failure)),
            Self::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                ErrorBody::new("FlanForge allocation timed out"),
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody::new("internal service error"),
            ),
        };
        (status, Json(body)).into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allocation_id: Option<AllocationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<AllocationState>,
}

impl ErrorBody {
    fn new(message: &'static str) -> Self {
        Self {
            error: Cow::Borrowed(message),
            allocation_id: None,
            state: None,
        }
    }
}

impl From<WorkerFailure> for ErrorBody {
    fn from(failure: WorkerFailure) -> Self {
        Self {
            error: failure
                .reason
                .map_or(Cow::Borrowed("FlanForge allocation failed"), Cow::Owned),
            allocation_id: failure.allocation_id,
            state: failure.state,
        }
    }
}
