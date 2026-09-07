use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, Table},
};

use crate::runner::MigrationStep;

/// `users` — web UI accounts. `password_hash` is NULL for provider-managed
/// rows; `oidc_subject` is the stable identity key for those.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0006_create_users"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        connection
            .execute(
                &Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Users::Id)
                            .big_integer()
                            .auto_increment()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Users::Username)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Users::PasswordHash).text().null())
                    .col(ColumnDef::new(Users::AuthSource).text().not_null())
                    .col(
                        ColumnDef::new(Users::OidcSubject)
                            .text()
                            .null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Users::CreatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::UpdatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Users {
    Table,
    Id,
    Username,
    PasswordHash,
    AuthSource,
    OidcSubject,
    CreatedAtUnix,
    UpdatedAtUnix,
}
