use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Index, Table},
};

use crate::runner::MigrationStep;

/// `events` — the durable "what has happened" log: state transitions, hot
/// claims and evictions, sweeps, reloads, edits, leaks. Bounded by retention,
/// never by hand.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0005_create_events"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(Events::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Events::Id)
                            .big_integer()
                            .auto_increment()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Events::OccurredAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Events::Kind).text().not_null())
                    .col(ColumnDef::new(Events::AllocationId).text().null())
                    .col(ColumnDef::new(Events::VmName).text().null())
                    .col(ColumnDef::new(Events::Profile).text().null())
                    .col(ColumnDef::new(Events::Actor).text().null())
                    .col(ColumnDef::new(Events::Payload).text().null())
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-events-kind")
                    .table(Events::Table)
                    .col(Events::Kind)
                    .col(Events::Id)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-events-allocation")
                    .table(Events::Table)
                    .col(Events::AllocationId)
                    .col(Events::Id)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-events-occurred")
                    .table(Events::Table)
                    .col(Events::OccurredAtUnix)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Events {
    Table,
    Id,
    OccurredAtUnix,
    Kind,
    AllocationId,
    VmName,
    Profile,
    Actor,
    Payload,
}
