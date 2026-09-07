use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Index, Table},
};

use crate::runner::MigrationStep;

/// `allocations` — every allocation ever admitted, terminal rows included;
/// this table is the queryable history the JSON files never were. Filterable
/// facts are columns; recorded verdicts (`origin`, `retention`) stay JSON.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0002_create_allocations"
    }

    // A table this wide is one column list; splitting it would obscure it.
    #[allow(clippy::too_many_lines)]
    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(Allocations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Allocations::Id)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Allocations::Profile).text().not_null())
                    .col(ColumnDef::new(Allocations::Repository).text().not_null())
                    .col(ColumnDef::new(Allocations::RunId).big_integer().not_null())
                    .col(
                        ColumnDef::new(Allocations::RunAttempt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Allocations::State).text().not_null())
                    .col(ColumnDef::new(Allocations::Mode).text().not_null())
                    .col(ColumnDef::new(Allocations::Origin).text().not_null())
                    .col(ColumnDef::new(Allocations::VmName).text().not_null())
                    .col(ColumnDef::new(Allocations::RunnerLabel).text().not_null())
                    .col(
                        ColumnDef::new(Allocations::VmCreated)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Allocations::RunnerId).big_integer().null())
                    .col(
                        ColumnDef::new(Allocations::CreatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Allocations::UpdatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Allocations::Error).text().null())
                    .col(
                        ColumnDef::new(Allocations::SizeCpuCount)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Allocations::SizeMemoryMb)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Allocations::SizeStorageMb)
                            .big_integer()
                            .null(),
                    )
                    .col(ColumnDef::new(Allocations::SourceName).text().null())
                    .col(ColumnDef::new(Allocations::SourceKind).text().null())
                    .col(ColumnDef::new(Allocations::SourceFingerprint).text().null())
                    .col(
                        ColumnDef::new(Allocations::SourceFallbackReason)
                            .text()
                            .null(),
                    )
                    .col(ColumnDef::new(Allocations::HotLane).text().null())
                    .col(ColumnDef::new(Allocations::HotRefusal).text().null())
                    .col(
                        ColumnDef::new(Allocations::HotAgeSeconds)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Allocations::WarmGeneration)
                            .big_integer()
                            .null(),
                    )
                    .col(ColumnDef::new(Allocations::Retention).text().null())
                    .col(ColumnDef::new(Allocations::TerminalReason).text().null())
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-allocations-state")
                    .table(Allocations::Table)
                    .col(Allocations::State)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-allocations-updated")
                    .table(Allocations::Table)
                    .col(Allocations::UpdatedAtUnix)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-allocations-attempt")
                    .table(Allocations::Table)
                    .col(Allocations::Repository)
                    .col(Allocations::RunId)
                    .col(Allocations::RunAttempt)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-allocations-profile-updated")
                    .table(Allocations::Table)
                    .col(Allocations::Profile)
                    .col(Allocations::UpdatedAtUnix)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Allocations {
    Table,
    Id,
    Profile,
    Repository,
    RunId,
    RunAttempt,
    State,
    Mode,
    Origin,
    VmName,
    RunnerLabel,
    VmCreated,
    RunnerId,
    CreatedAtUnix,
    UpdatedAtUnix,
    Error,
    SizeCpuCount,
    SizeMemoryMb,
    SizeStorageMb,
    SourceName,
    SourceKind,
    SourceFingerprint,
    SourceFallbackReason,
    HotLane,
    HotRefusal,
    HotAgeSeconds,
    WarmGeneration,
    Retention,
    TerminalReason,
}
