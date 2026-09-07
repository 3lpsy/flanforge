//! HTTP router composition for the `FlanForge` control plane.

mod listener;
mod trace;
mod webui;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};
use flanforge_routes::{
    AppState, OperatorState, cancel, create, ensure_authenticated, ensure_local_peer, health,
    operator_cancel, operator_hot, operator_hot_retire, operator_list, operator_reap,
    operator_status, operator_webui_user_create, operator_webui_user_delete,
    operator_webui_user_reset_password, operator_webui_users_list, status,
};

pub use listener::{BoundedListener, with_peer_info};
use trace::trace_request;
pub use webui::build_webui_router;

/// The workflow-facing surface. Authentication is the default; the public
/// branch is the declared exception.
pub fn build_router(state: AppState, request_body_limit_bytes: usize) -> Router {
    allocation_surface(
        public_routes(),
        protected_routes(),
        state,
        request_body_limit_bytes,
    )
}

/// The allowlist: the only routes served without a verified workflow identity.
fn public_routes() -> Router<AppState> {
    Router::new().route("/healthz", get(health))
}

fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/allocations", post(create))
        .route("/v1/allocations/{id}", get(status).delete(cancel))
}

/// Verification covers the protected branch as one unit, so a route added to
/// it cannot skip it. The shared layers are attached after the branches merge,
/// so registration order within either branch is not load-bearing.
fn allocation_surface(
    public: Router<AppState>,
    protected: Router<AppState>,
    state: AppState,
    request_body_limit_bytes: usize,
) -> Router {
    protected
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            ensure_authenticated,
        ))
        .merge(public)
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
        .route("/v1/operator/hot", get(operator_hot))
        .route("/v1/operator/hot/{name}", post(operator_hot_retire))
        .route("/v1/operator/status", get(operator_status))
        .route("/v1/operator/reap", post(operator_reap))
        .route(
            "/v1/operator/webui/users",
            get(operator_webui_users_list).post(operator_webui_user_create),
        )
        .route(
            "/v1/operator/webui/users/{name}",
            delete(operator_webui_user_delete),
        )
        .route(
            "/v1/operator/webui/users/{name}/password",
            post(operator_webui_user_reset_password),
        )
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
