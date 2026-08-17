use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
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
    Busy,
    Worker,
    Timeout,
    Internal,
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Unavailable => tracing::warn!("OIDC provider is unavailable"),
            AuthError::MissingCredentials | AuthError::InvalidToken => {
                tracing::debug!("allocation authentication rejected");
            }
        }
        match error {
            AuthError::Unavailable => Self::IdentityProvider,
            AuthError::MissingCredentials | AuthError::InvalidToken => Self::Authentication,
        }
    }
}

impl From<ManagerError> for ApiError {
    fn from(error: ManagerError) -> Self {
        match &error {
            ManagerError::DuplicateAllocation(_)
            | ManagerError::Transition(_)
            | ManagerError::Store(_) => tracing::error!(%error, "allocation manager failed"),
            ManagerError::Worker(_) => tracing::warn!(%error, "allocation worker failed"),
            ManagerError::Authorization(_) => {
                tracing::warn!(reason = %error, "allocation authorization rejected");
            }
            _ => tracing::debug!(reason = %error, "allocation request rejected"),
        }
        match error {
            ManagerError::Authorization(_) | ManagerError::RequestExceedsProfile => Self::Forbidden,
            ManagerError::NotFound(_) => Self::NotFound,
            ManagerError::Busy { .. } => Self::Busy,
            ManagerError::ShuttingDown => Self::ServiceUnavailable,
            ManagerError::UnknownProfile(_)
            | ManagerError::InvalidVmName(_)
            | ManagerError::InvalidRunnerLabel(_)
            | ManagerError::InvalidRequest
            | ManagerError::InvalidRunnerId => Self::BadRequest,
            ManagerError::Worker(_) => Self::Worker,
            ManagerError::DuplicateAllocation(_)
            | ManagerError::Transition(_)
            | ManagerError::Store(_) => Self::Internal,
        }
    }
}

impl From<WorkerError> for ApiError {
    fn from(error: WorkerError) -> Self {
        tracing::warn!(%error, "allocation worker failed");
        Self::Worker
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Authentication => (StatusCode::UNAUTHORIZED, "authentication required"),
            Self::IdentityProvider => (
                StatusCode::SERVICE_UNAVAILABLE,
                "identity provider unavailable",
            ),
            Self::ServiceUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "service is shutting down")
            }
            Self::BadRequest => (StatusCode::BAD_REQUEST, "invalid request"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "request is not authorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "allocation not found"),
            Self::Busy => (StatusCode::CONFLICT, "FlanForge is busy"),
            Self::Worker => (StatusCode::BAD_GATEWAY, "FlanForge allocation failed"),
            Self::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "FlanForge allocation timed out",
            ),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal service error"),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
}
