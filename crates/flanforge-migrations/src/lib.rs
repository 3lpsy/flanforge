//! Owns the daemon's `SQLite` schema: connection options and the ordered
//! migration set. Entities and stores live in `flanforge-orm`.

mod connect;
mod m0001_create_meta;
mod m0002_create_allocations;
mod m0003_create_hot_guests;
mod m0004_create_warm_images;
mod m0005_create_events;
mod m0006_create_users;
mod m0007_create_sessions;
mod migrator;
mod runner;

pub use connect::{DbOpenError, connect_and_migrate};
pub use runner::MigrationStep;

#[cfg(test)]
mod tests;
