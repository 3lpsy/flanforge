use axum::{http::StatusCode, response::Response};
use flanforge_extractors::error_response;
use flanforge_handlers::WebuiFault;

/// Maps an action's refusal to its one status and safe message.
#[must_use]
pub fn fault_response(fault: &WebuiFault) -> Response {
    let status = match fault {
        WebuiFault::Unauthenticated => StatusCode::UNAUTHORIZED,
        WebuiFault::Forbidden => StatusCode::FORBIDDEN,
        WebuiFault::NotFound => StatusCode::NOT_FOUND,
        WebuiFault::Conflict(_) => StatusCode::CONFLICT,
        WebuiFault::Invalid(_) => StatusCode::BAD_REQUEST,
        WebuiFault::Rejected(_) => StatusCode::UNPROCESSABLE_ENTITY,
        WebuiFault::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, &fault.to_string())
}
