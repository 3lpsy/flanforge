use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Table},
};

use crate::runner::MigrationStep;

/// `warm_images` — one row per profile's warm image. `previous` is the
/// retained rollback generation, read back whole, so it stays JSON.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0004_create_warm_images"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(WarmImages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WarmImages::Profile)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WarmImages::WarmTemplate).text().not_null())
                    .col(
                        ColumnDef::new(WarmImages::Generation)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WarmImages::BaseFingerprint)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WarmImages::ProducedBy).text().not_null())
                    .col(
                        ColumnDef::new(WarmImages::ProducedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WarmImages::State).text().not_null())
                    .col(ColumnDef::new(WarmImages::Previous).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum WarmImages {
    Table,
    Profile,
    WarmTemplate,
    Generation,
    BaseFingerprint,
    ProducedBy,
    ProducedAtUnix,
    State,
    Previous,
}
