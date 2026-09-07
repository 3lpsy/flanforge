use crate::runner::MigrationStep;
use crate::{
    m0001_create_meta, m0002_create_allocations, m0003_create_hot_guests, m0004_create_warm_images,
    m0005_create_events, m0006_create_users, m0007_create_sessions,
};

/// The ordered migration set. Shipped migrations are immutable: schema changes
/// get a new file that accounts for existing data.
pub(crate) const MIGRATIONS: &[&dyn MigrationStep] = &[
    &m0001_create_meta::Migration,
    &m0002_create_allocations::Migration,
    &m0003_create_hot_guests::Migration,
    &m0004_create_warm_images::Migration,
    &m0005_create_events::Migration,
    &m0006_create_users::Migration,
    &m0007_create_sessions::Migration,
];
