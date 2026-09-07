use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use flanforge_wire::WebuiErrorBody;

/// The one `/api/v1` error shape: a JSON envelope, never a redirect.
#[must_use]
pub fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(WebuiErrorBody {
            error: message.to_owned(),
        }),
    )
        .into_response()
}
