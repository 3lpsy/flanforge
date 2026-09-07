use std::{path::Path, time::Duration};

use sea_orm::{
    DatabaseConnection, SqlxSqliteConnector,
    sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

use crate::{migrator::MIGRATIONS, runner};

#[derive(Debug, Error)]
pub enum DbOpenError {
    #[error("cannot open database {path}: {source}")]
    Open {
        path: String,
        source: sea_orm::sqlx::Error,
    },
    #[error("database migration failed: {0}")]
    Migrate(sea_orm::DbErr),
}

/// Opens the database, creating it when missing, and applies every pending
/// migration before returning. WAL keeps readers unblocked while the daemon
/// writes; a failure here must abort startup before any listener binds.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or a migration fails.
pub async fn connect_and_migrate(db_path: &Path) -> Result<DatabaseConnection, DbOpenError> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    // Connections to a local file are kept for the process lifetime: there is
    // no server to be polite to, and reconnect churn only adds latency.
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .map_err(|source| DbOpenError::Open {
            path: db_path.display().to_string(),
            source,
        })?;
    let connection = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);
    runner::apply_pending(&connection, MIGRATIONS)
        .await
        .map_err(DbOpenError::Migrate)?;
    Ok(connection)
}
