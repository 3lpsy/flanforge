//! HTTP router composition for the `FlanForge` control plane.

mod listener;
mod trace;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};
use flanforge_routes::{
    AppState, OperatorState, cancel, create, ensure_local_peer, health, operator_cancel,
    operator_list, operator_reap, operator_status, status,
};

pub use listener::{BoundedListener, with_peer_info};
use trace::trace_request;

pub fn build_router(state: AppState, request_body_limit_bytes: usize) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/allocations", post(create))
        .route("/v1/allocations/{id}", get(status).delete(cancel))
        .layer(axum::middleware::from_fn(trace_request))
        .layer(DefaultBodyLimit::max(request_body_limit_bytes))
        .with_state(state)
}

/// The host-only surface, peer-checked as one unit.
pub fn build_operator_router(state: OperatorState, request_body_limit_bytes: usize) -> Router {
    operator_surface(operator_routes(), state, request_body_limit_bytes)
}

fn operator_routes() -> Router<OperatorState> {
    Router::new()
        .route("/v1/operator/allocations", get(operator_list))
        .route("/v1/operator/allocations/{id}", delete(operator_cancel))
        .route("/v1/operator/status", get(operator_status))
        .route("/v1/operator/reap", post(operator_reap))
}

/// Applies the peer check to the whole route set, so a route added to it
/// cannot silently skip the check.
fn operator_surface(
    routes: Router<OperatorState>,
    state: OperatorState,
    request_body_limit_bytes: usize,
) -> Router {
    routes
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            ensure_local_peer,
        ))
        .layer(axum::middleware::from_fn(trace_request))
        .layer(DefaultBodyLimit::max(request_body_limit_bytes))
        .with_state(state)
}

#[cfg(test)]
mod tests;
