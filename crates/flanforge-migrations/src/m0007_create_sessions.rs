use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, DeriveIden,
    sea_query::{ColumnDef, ForeignKey, ForeignKeyAction, Index, Table},
};

use crate::runner::MigrationStep;

use crate::m0006_create_users::Users;

/// `sessions` — web UI logins. Only the SHA-256 of the cookie token is
/// stored, so a database read never yields a usable cookie.
pub struct Migration;

#[async_trait]
impl MigrationStep for Migration {
    fn name(&self) -> &'static str {
        "m0007_create_sessions"
    }

    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbErr> {
        let mut fk_user = ForeignKey::create();
        fk_user
            .name("fk-sessions-user")
            .from_tbl(Sessions::Table)
            .from_col(Sessions::UserId)
            .to_tbl(Users::Table)
            .to_col(Users::Id)
            .on_delete(ForeignKeyAction::Cascade)
            .on_update(ForeignKeyAction::Cascade);
        connection
            .execute(
                &Table::create()
                    .table(Sessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sessions::Id)
                            .big_integer()
                            .auto_increment()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Sessions::TokenHash)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Sessions::UserId).big_integer().not_null())
                    .col(ColumnDef::new(Sessions::Kind).text().not_null())
                    .col(
                        ColumnDef::new(Sessions::CreatedAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Sessions::ExpiresAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Sessions::LastSeenAtUnix)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(&mut fk_user)
                    .to_owned(),
            )
            .await?;
        connection
            .execute(
                &Index::create()
                    .name("idx-sessions-expires")
                    .table(Sessions::Table)
                    .col(Sessions::ExpiresAtUnix)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Sessions {
    Table,
    Id,
    TokenHash,
    UserId,
    Kind,
    CreatedAtUnix,
    ExpiresAtUnix,
    LastSeenAtUnix,
}
