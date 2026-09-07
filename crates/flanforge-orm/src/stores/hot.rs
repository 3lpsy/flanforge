use async_trait::async_trait;
use flanforge_core::{HotGuest, VmName};
use flanforge_store::{HotGuestStore, StoreError};
use sea_orm::{DatabaseConnection, EntityTrait, IntoActiveModel, sea_query::OnConflict};

use crate::{
    entities::hot_guest::{Column, Entity},
    hot_guest_from_model, hot_guest_to_model,
};

#[derive(Clone, Debug)]
pub struct SqliteHotGuestStore {
    connection: DatabaseConnection,
}

impl SqliteHotGuestStore {
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
impl HotGuestStore for SqliteHotGuestStore {
    async fn load_all(&self) -> Result<Vec<HotGuest>, StoreError> {
        let rows = Entity::find()
            .all(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(rows
            .iter()
            .filter_map(|row| match hot_guest_from_model(row) {
                Ok(guest) => Some(guest),
                Err(error) => {
                    tracing::error!(vm_name = %row.vm_name, %error, "hot guest row is unreadable; skipped");
                    None
                }
            })
            .collect())
    }

    async fn load(&self, vm_name: &VmName) -> Result<Option<HotGuest>, StoreError> {
        let row = Entity::find_by_id(vm_name.as_str())
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        match row {
            None => Ok(None),
            Some(row) => hot_guest_from_model(&row)
                .map(Some)
                .map_err(|error| StoreError::Db {
                    message: error.to_string(),
                }),
        }
    }

    async fn save(&self, guest: &HotGuest) -> Result<(), StoreError> {
        let model = hot_guest_to_model(guest).map_err(|error| StoreError::Db {
            message: error.to_string(),
        })?;
        Entity::insert(model.into_active_model())
            .on_conflict(
                OnConflict::column(Column::VmName)
                    .update_columns(non_key_columns())
                    .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }

    async fn remove(&self, vm_name: &VmName) -> Result<(), StoreError> {
        Entity::delete_by_id(vm_name.as_str())
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }
}

fn non_key_columns() -> Vec<Column> {
    use sea_orm::Iterable;
    Column::iter()
        .filter(|column| !matches!(column, Column::VmName))
        .collect()
}
