//! Axum extractors for the web UI surface. `Body<T>` validates
//! unconditionally inside extraction, so a route physically cannot skip
//! validation; identity extractors fail closed when the middleware that
//! attaches identity was somehow missed.

mod body;
mod identity;
mod reject;

pub use body::Body;
pub use identity::{Identity, RequireUser, ResolvedSession, WebuiIdentity};
pub use reject::error_response;

#[cfg(test)]
mod tests;
