//! Axum glue for the web UI: identity middleware, tier guards, CSRF, the
//! `/api/v1` route functions, and the embedded `/ui` asset server. Route
//! *assembly* — which routes sit in which tier — lives in `flanforge-router`.

mod api;
mod assets;
mod fault;
mod guards;
mod identity;
mod sse;

pub use api::{actions, config, meta, oidc, session, state, users};
pub use assets::assets_router;
pub use fault::fault_response;
pub use guards::{CSRF_HEADER, ensure_csrf, require_auth, require_read};
pub use identity::attach_identity;
pub use sse::{SSE_KEEP_ALIVE, allocation_stream, events_stream, logs_stream};
