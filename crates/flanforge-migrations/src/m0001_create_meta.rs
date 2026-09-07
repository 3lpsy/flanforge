use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Table},
};

use crate::runner::MigrationStep;

/// `meta` — installation facts: import completion, the state dir this
/// database belongs to.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0001_create_meta"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(Meta::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Meta::Key).text().not_null().primary_key())
                    .col(ColumnDef::new(Meta::Value).text().not_null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Meta {
    Table,
    Key,
    Value,
}
