use async_trait::async_trait;
use flanforge_core::Allocation;
use flanforge_store::{AllocationStore, StoreError};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder,
    QuerySelect, sea_query::OnConflict,
};

use crate::{
    allocation_from_model, allocation_to_model,
    entities::allocation::{Column, Entity},
};

/// Serde tokens of the terminal states, for SQL filters. Kept next to a test
/// asserting it matches `AllocationState::is_terminal`.
pub(super) const TERMINAL_STATE_TOKENS: [&str; 3] = ["completed", "failed", "cancelled"];

#[derive(Clone, Debug)]
pub struct SqliteAllocationStore {
    connection: DatabaseConnection,
}

impl SqliteAllocationStore {
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

/// Decodes rows, skipping and logging anything that cannot be trusted — the
/// database analogue of quarantining a corrupt JSON record.
fn decode_rows(rows: &[crate::entities::allocation::Model]) -> Vec<Allocation> {
    rows.iter()
        .filter_map(|row| match allocation_from_model(row) {
            Ok(allocation) => Some(allocation),
            Err(error) => {
                tracing::error!(id = %row.id, %error, "allocation row is unreadable; skipped");
                None
            }
        })
        .collect()
}

#[async_trait]
impl AllocationStore for SqliteAllocationStore {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError> {
        // The newest `limit` rows whatever their state, exactly the window the
        // mtime-ordered JSON store loaded...
        let newest = Entity::find()
            .order_by_desc(Column::UpdatedAtUnix)
            .order_by_desc(Column::Id)
            .limit(u64::try_from(limit).unwrap_or(u64::MAX))
            .all(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        // ...plus every non-terminal row beyond it: unfinished work can still
        // own a VM and a runner registration, so it is never dropped.
        let live = Entity::find()
            .filter(Column::State.is_not_in(TERMINAL_STATE_TOKENS))
            .all(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        let mut allocations = decode_rows(&newest);
        for allocation in decode_rows(&live) {
            if !allocations.iter().any(|seen| seen.id == allocation.id) {
                allocations.push(allocation);
            }
        }
        Ok(allocations)
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError> {
        if let Err(error) = validator::Validate::validate(allocation) {
            tracing::error!(%error, "refusing to persist invalid allocation");
            return Err(StoreError::Db {
                message: "allocation is structurally invalid".to_owned(),
            });
        }
        let model = allocation_to_model(allocation).map_err(|error| StoreError::Db {
            message: error.to_string(),
        })?;
        Entity::insert(model.into_active_model())
            .on_conflict(
                OnConflict::column(Column::Id)
                    .update_columns(non_key_columns())
                    .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }
}

fn non_key_columns() -> Vec<Column> {
    use sea_orm::Iterable;
    Column::iter()
        .filter(|column| !matches!(column, Column::Id))
        .collect()
}

/// Compile-time-adjacent guard: the token list above must track the enum.
#[cfg(test)]
pub(super) fn terminal_tokens_match_the_state_machine() -> bool {
    use flanforge_core::AllocationState;
    let states = [
        AllocationState::Completed,
        AllocationState::Failed,
        AllocationState::Cancelled,
    ];
    states.iter().all(|state| {
        serde_json::to_value(state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .is_some_and(|token| TERMINAL_STATE_TOKENS.contains(&token.as_str()))
    }) && states.iter().all(|state| state.is_terminal())
}
