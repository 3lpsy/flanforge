//! The web UI's actions. Handlers are plain async functions over
//! `WebuiServices` and wire DTOs — no axum types — so they are testable
//! without HTTP and reusable by any transport.

mod error;
mod services;

pub mod allocations;
pub mod config;
pub mod events;
pub mod hot;
pub mod meta;
pub mod reaper;
pub mod session;
pub mod status;
pub mod users;

mod views;

pub use error::WebuiFault;
pub use services::WebuiServices;

#[cfg(test)]
mod tests_support;
