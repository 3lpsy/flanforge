use std::path::Path;

use sea_orm::{ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use thiserror::Error;

use crate::entities::meta;

pub(super) const IMPORT_COMPLETED_KEY: &str = "json_import_completed_at";
const STATE_DIR_KEY: &str = "state_dir";

#[derive(Debug, Error)]
pub enum MetaError {
    #[error("database operation failed: {0}")]
    Db(#[from] DbErr),
    #[error(
        "this database belongs to state dir {recorded}, not {configured}: two daemons must not \
         share one db_path"
    )]
    ForeignStateDir {
        recorded: String,
        configured: String,
    },
}

/// Binds the database to one state directory on first boot and refuses a
/// mismatch after that. `instance.lock` cannot see two daemons pointed at one
/// overridden `db_path`; this can.
///
/// # Errors
///
/// Returns an error on a database failure or a state-dir mismatch.
pub async fn ensure_state_dir_identity(
    connection: &DatabaseConnection,
    state_dir: &Path,
) -> Result<(), MetaError> {
    let configured = state_dir.display().to_string();
    match read(connection, STATE_DIR_KEY).await? {
        None => {
            write(connection, STATE_DIR_KEY, &configured).await?;
            Ok(())
        }
        Some(recorded) if recorded == configured => Ok(()),
        Some(recorded) => Err(MetaError::ForeignStateDir {
            recorded,
            configured,
        }),
    }
}

pub(super) async fn read(
    connection: &DatabaseConnection,
    key: &str,
) -> Result<Option<String>, DbErr> {
    Ok(meta::Entity::find()
        .filter(meta::Column::Key.eq(key))
        .one(connection)
        .await?
        .map(|row| row.value))
}

pub(super) async fn write(
    connection: &DatabaseConnection,
    key: &str,
    value: &str,
) -> Result<(), DbErr> {
    use sea_orm::sea_query::OnConflict;
    meta::Entity::insert(meta::ActiveModel {
        key: Set(key.to_owned()),
        value: Set(value.to_owned()),
    })
    .on_conflict(
        OnConflict::column(meta::Column::Key)
            .update_column(meta::Column::Value)
            .to_owned(),
    )
    .exec(connection)
    .await?;
    Ok(())
}
