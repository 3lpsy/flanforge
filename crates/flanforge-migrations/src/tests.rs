use sea_orm::{ConnectionTrait, Statement};

use super::*;

async fn table_names(connection: &sea_orm::DatabaseConnection) -> Vec<String> {
    let rows = connection
        .query_all_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        ))
        .await
        .unwrap_or_else(|error| unreachable!("query sqlite_master: {error}"));
    rows.iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .collect()
}

#[tokio::test]
async fn connect_creates_the_file_and_applies_every_migration() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let db_path = directory.path().join("flanforge.db");
    let connection = connect_and_migrate(&db_path)
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));

    assert!(db_path.is_file());
    let tables = table_names(&connection).await;
    for table in [
        "meta",
        "allocations",
        "hot_guests",
        "warm_images",
        "events",
        "users",
        "sessions",
    ] {
        assert!(tables.iter().any(|name| name == table), "missing {table}");
    }
    let recorded = connection
        .query_all_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT version FROM seaql_migrations ORDER BY version",
        ))
        .await
        .unwrap_or_else(|error| unreachable!("query seaql_migrations: {error}"));
    assert_eq!(recorded.len(), crate::migrator::MIGRATIONS.len());
}

#[tokio::test]
async fn a_second_connect_is_idempotent() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let db_path = directory.path().join("flanforge.db");
    drop(
        connect_and_migrate(&db_path)
            .await
            .unwrap_or_else(|error| unreachable!("first connect: {error}")),
    );
    drop(
        connect_and_migrate(&db_path)
            .await
            .unwrap_or_else(|error| unreachable!("second connect: {error}")),
    );
}
