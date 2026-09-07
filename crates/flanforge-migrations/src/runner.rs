use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr,
    sea_query::{ColumnDef, Query, Table},
};

/// One schema step. Shipped migrations are immutable: schema changes get a
/// new file that accounts for existing data.
#[async_trait]
pub trait MigrationStep: Send + Sync {
    /// Stable identity, recorded in the bookkeeping table.
    fn name(&self) -> &'static str;

    /// Applies the step. Runs at most once per database.
    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr>;
}

/// Bookkeeping table, shaped like sea-orm-migration's so a database created
/// while this crate still used that framework keeps its history.
#[derive(sea_orm::DeriveIden)]
enum SeaqlMigrations {
    Table,
    Version,
    AppliedAt,
}

/// Applies every migration not yet recorded, in order, recording each as it
/// lands. The framework this replaces forced sea-orm's default features —
/// and a CoreFoundation link — into the Darwin cross-build; this does not.
pub(crate) async fn apply_pending(
    connection: &DatabaseConnection,
    migrations: &[&dyn MigrationStep],
) -> Result<(), DbErr> {
    connection
        .execute(
            &Table::create()
                .table(SeaqlMigrations::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(SeaqlMigrations::Version)
                        .text()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(SeaqlMigrations::AppliedAt)
                        .big_integer()
                        .not_null(),
                )
                .to_owned(),
        )
        .await?;
    let applied = connection
        .query_all(
            &Query::select()
                .column(SeaqlMigrations::Version)
                .from(SeaqlMigrations::Table)
                .to_owned(),
        )
        .await?
        .iter()
        .filter_map(|row| row.try_get::<String>("", "version").ok())
        .collect::<std::collections::BTreeSet<_>>();
    for migration in migrations {
        if applied.contains(migration.name()) {
            continue;
        }
        tracing::info!(migration = migration.name(), "applying migration");
        migration.up(connection).await?;
        connection
            .execute(
                &Query::insert()
                    .into_table(SeaqlMigrations::Table)
                    .columns([SeaqlMigrations::Version, SeaqlMigrations::AppliedAt])
                    .values([migration.name().into(), now_unix().into()])
                    .map_err(|error| DbErr::Custom(error.to_string()))?
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
