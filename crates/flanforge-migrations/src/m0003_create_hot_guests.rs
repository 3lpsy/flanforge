use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Index, Table},
};

use crate::runner::MigrationStep;

/// `hot_guests` — one row per retained machine. `claimed_by` has no foreign
/// key: allocation rows are prunable history, the claim is a live fact.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0003_create_hot_guests"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(HotGuests::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(HotGuests::VmName)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(HotGuests::Profile).text().not_null())
                    .col(ColumnDef::new(HotGuests::Lane).text().not_null())
                    .col(ColumnDef::new(HotGuests::State).text().not_null())
                    .col(
                        ColumnDef::new(HotGuests::SizeCpuCount)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::SizeMemoryMb)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::SizeStorageMb)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(HotGuests::SourceName).text().not_null())
                    .col(ColumnDef::new(HotGuests::SourceKind).text().not_null())
                    .col(ColumnDef::new(HotGuests::SourceFingerprint).text().null())
                    .col(
                        ColumnDef::new(HotGuests::SourceFallbackReason)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::WarmGeneration)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::AgeLimitSeconds)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::BootedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::UpdatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(HotGuests::JobsServed)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(HotGuests::ClaimedBy).text().null())
                    .col(ColumnDef::new(HotGuests::DrainReason).text().null())
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-hot-guests-serving")
                    .table(HotGuests::Table)
                    .col(HotGuests::Profile)
                    .col(HotGuests::Lane)
                    .col(HotGuests::State)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum HotGuests {
    Table,
    VmName,
    Profile,
    Lane,
    State,
    SizeCpuCount,
    SizeMemoryMb,
    SizeStorageMb,
    SourceName,
    SourceKind,
    SourceFingerprint,
    SourceFallbackReason,
    WarmGeneration,
    AgeLimitSeconds,
    BootedAtUnix,
    UpdatedAtUnix,
    JobsServed,
    ClaimedBy,
    DrainReason,
}
