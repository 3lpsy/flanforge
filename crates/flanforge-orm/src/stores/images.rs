use async_trait::async_trait;
use flanforge_core::{ProfileName, WarmImageRecord};
use flanforge_store::{StoreError, WarmImageStore};
use sea_orm::{DatabaseConnection, EntityTrait, IntoActiveModel, sea_query::OnConflict};

use crate::{
    entities::warm_image::{Column, Entity},
    warm_image_from_model, warm_image_to_model,
};

#[derive(Clone, Debug)]
pub struct SqliteWarmImageStore {
    connection: DatabaseConnection,
}

impl SqliteWarmImageStore {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    /// Opens (creating and migrating if needed) the database at `db_path` and
    /// wraps it. The daemon shares one connection across stores instead;
    /// this is for tools and tests that need just one store.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened or migrated.
    pub async fn open(db_path: &std::path::Path) -> Result<Self, StoreError> {
        let connection = flanforge_migrations::connect_and_migrate(db_path)
            .await
            .map_err(|error| StoreError::Db {
                message: error.to_string(),
            })?;
        Ok(Self::new(connection))
    }
}

fn db_error(source: &sea_orm::DbErr) -> StoreError {
    StoreError::Db {
        message: source.to_string(),
    }
}

#[async_trait]
impl WarmImageStore for SqliteWarmImageStore {
    async fn load_all(&self) -> Result<Vec<WarmImageRecord>, StoreError> {
        let rows = Entity::find()
            .all(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(rows
            .iter()
            .filter_map(|row| match warm_image_from_model(row) {
                Ok(record) => Some(record),
                Err(error) => {
                    tracing::error!(profile = %row.profile, %error, "warm image row is unreadable; skipped");
                    None
                }
            })
            .collect())
    }

    async fn load(&self, profile: &ProfileName) -> Result<Option<WarmImageRecord>, StoreError> {
        let row = Entity::find_by_id(profile.as_str())
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        match row {
            None => Ok(None),
            Some(row) => warm_image_from_model(&row)
                .map(Some)
                .map_err(|error| StoreError::Db {
                    message: error.to_string(),
                }),
        }
    }

    async fn save(&self, record: &WarmImageRecord) -> Result<(), StoreError> {
        let model = warm_image_to_model(record).map_err(|error| StoreError::Db {
            message: error.to_string(),
        })?;
        Entity::insert(model.into_active_model())
            .on_conflict(
                OnConflict::column(Column::Profile)
                    .update_columns(non_key_columns())
                    .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }

    async fn remove(&self, profile: &ProfileName) -> Result<(), StoreError> {
        Entity::delete_by_id(profile.as_str())
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }
}

fn non_key_columns() -> Vec<Column> {
    use sea_orm::Iterable;
    Column::iter()
        .filter(|column| !matches!(column, Column::Profile))
        .collect()
}
