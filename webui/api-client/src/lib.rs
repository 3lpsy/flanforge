//! Same-origin JSON fetch and SSE helpers for the flanforge web UI. Cookies
//! ride along automatically (default `credentials: same-origin`); never set
//! CORS mode. Every non-GET carries the CSRF header the server requires.

mod fetch;
mod sse;

pub use fetch::{ApiError, get_json, send_empty, send_json, sleep_ms};
pub use flanforge_wire as wire;
pub use sse::SseStream;
