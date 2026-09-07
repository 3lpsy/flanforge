use std::path::PathBuf;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{any, delete, get, post},
};
use flanforge_extractors::error_response;
use flanforge_handlers::WebuiServices;
use flanforge_webui_routes::{
    actions, allocation_stream, assets_router, attach_identity, config, ensure_csrf, events_stream,
    logs_stream, meta, oidc, require_auth, require_read, session, state, users,
};

use crate::trace::trace_request;

/// The browser surface: `/api/v1` in three tiers plus the `/ui` SPA. Merged
/// onto the public router — never the operator branch — and mounted only when
/// `webui.enabled`.
///
/// Tier invariants, guarded as units so a route cannot skip its guard:
/// - public: login, logout, and the meta document the login page needs;
/// - readonly: state views, open to anonymous callers iff
///   `webui.public_read_only`;
/// - mutating: everything else, including sensitive reads (accounts, and
///   later logs and configuration), behind a session plus the CSRF wall.
pub fn build_webui_router(
    services: WebuiServices,
    request_body_limit_bytes: usize,
    dev_dist_dir: Option<PathBuf>,
) -> Router {
    let public = Router::new()
        .route("/api/v1/meta", get(meta::get))
        .route(
            "/api/v1/session",
            post(session::login).delete(session::logout),
        )
        .route("/api/v1/oidc/login", get(oidc::login))
        .route("/api/v1/oidc/callback", get(oidc::callback));
    // The tier is empty until the state views land; `route_layer` on an
    // empty router is a startup panic, so the guard waits for its routes.
    let mut readonly = readonly_routes();
    if readonly.has_routes() {
        readonly = readonly.route_layer(axum::middleware::from_fn_with_state(
            services.clone(),
            require_read,
        ));
    }
    let mutating = mutating_routes()
        .route_layer(axum::middleware::from_fn(ensure_csrf))
        .route_layer(axum::middleware::from_fn(require_auth));
    public
        .merge(readonly)
        .merge(mutating)
        // Unknown /api/v1 paths answer JSON here; they must never fall
        // through to another surface's fallback.
        .route("/api/v1", any(unknown_api))
        .route("/api/v1/{*rest}", any(unknown_api))
        .layer(axum::middleware::from_fn_with_state(
            services.clone(),
            attach_identity,
        ))
        .layer(axum::middleware::from_fn(trace_request))
        .layer(DefaultBodyLimit::max(request_body_limit_bytes))
        .with_state(services)
        // Assets ride outside the identity layer: a static file costs no
        // database read.
        .merge(assets_router(dev_dist_dir))
}

fn readonly_routes() -> Router<WebuiServices> {
    Router::new()
        .route("/api/v1/status", get(state::status))
        .route("/api/v1/allocations", get(state::allocations))
        .route(
            "/api/v1/allocations/history",
            get(state::allocation_history),
        )
        .route("/api/v1/allocations/{id}", get(state::allocation_detail))
        .route("/api/v1/hot", get(state::hot))
        .route("/api/v1/warm", get(state::warm))
        .route("/api/v1/sweeps", get(state::sweeps))
        .route("/api/v1/events", get(state::events))
        .route("/api/v1/events/stream", get(events_stream))
        .route("/api/v1/allocations/{id}/stream", get(allocation_stream))
}

fn mutating_routes() -> Router<WebuiServices> {
    Router::new()
        .route("/api/v1/users", get(users::list).post(users::create))
        .route("/api/v1/users/{id}", delete(users::delete))
        .route("/api/v1/users/{id}/password", post(users::set_password))
        .route(
            "/api/v1/allocations/{id}",
            delete(actions::cancel_allocation),
        )
        .route("/api/v1/hot/{name}/retire", post(actions::hot_retire))
        .route("/api/v1/reap", post(actions::reap))
        // The daemon's own log is a sensitive read; it lives with the
        // mutations, never in the readonly tier.
        .route("/api/v1/logs/stream", get(logs_stream))
        // Configuration reads sit here too: the file carries authorization
        // policy and infrastructure paths.
        .route("/api/v1/config", get(config::get).patch(config::patch))
        .route(
            "/api/v1/profiles/{name}",
            axum::routing::put(config::profile_put).delete(config::profile_delete),
        )
}

async fn unknown_api() -> axum::response::Response {
    error_response(StatusCode::NOT_FOUND, "unknown API path")
}
