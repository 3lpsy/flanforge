use flanforge_core::unix_time;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};

use super::allocations::TERMINAL_STATE_TOKENS;
use crate::entities::{allocation, event};

/// Hard retention caps, deliberately constants rather than configuration: the
/// point is that nobody ever has to prune by hand.
const EVENTS_MAX_ROWS: u64 = 50_000;
const EVENTS_MAX_AGE_SECONDS: u64 = 90 * 24 * 3_600;
const ALLOCATIONS_MAX_TERMINAL_ROWS: u64 = 10_000;
const ALLOCATIONS_MAX_AGE_SECONDS: u64 = 180 * 24 * 3_600;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PruneOutcome {
    pub events_deleted: u64,
    pub allocations_deleted: u64,
}

/// Bounds the history tables by age and row count. Non-terminal allocations
/// are never touched: unfinished work can still own a VM.
///
/// # Errors
///
/// Returns an error when a delete does not commit.
pub async fn prune_history(connection: &DatabaseConnection) -> Result<PruneOutcome, DbErr> {
    let now = unix_time();
    let mut outcome = PruneOutcome::default();

    let age_floor = to_i64(now.saturating_sub(EVENTS_MAX_AGE_SECONDS));
    outcome.events_deleted += event::Entity::delete_many()
        .filter(event::Column::OccurredAtUnix.lt(age_floor))
        .exec(connection)
        .await?
        .rows_affected;
    if let Some(cutoff) = event::Entity::find()
        .select_only()
        .column(event::Column::Id)
        .order_by_desc(event::Column::Id)
        .offset(EVENTS_MAX_ROWS)
        .limit(1)
        .into_tuple::<i64>()
        .one(connection)
        .await?
    {
        outcome.events_deleted += event::Entity::delete_many()
            .filter(event::Column::Id.lte(cutoff))
            .exec(connection)
            .await?
            .rows_affected;
    }

    let age_floor = to_i64(now.saturating_sub(ALLOCATIONS_MAX_AGE_SECONDS));
    let terminal = allocation::Column::State.is_in(TERMINAL_STATE_TOKENS);
    outcome.allocations_deleted += allocation::Entity::delete_many()
        .filter(terminal.clone())
        .filter(allocation::Column::UpdatedAtUnix.lt(age_floor))
        .exec(connection)
        .await?
        .rows_affected;
    // Row-count cap: the cutoff is strict, so timestamp ties err toward
    // keeping rows rather than deleting past the cap.
    if let Some(cutoff) = allocation::Entity::find()
        .select_only()
        .column(allocation::Column::UpdatedAtUnix)
        .filter(terminal.clone())
        .order_by_desc(allocation::Column::UpdatedAtUnix)
        .offset(ALLOCATIONS_MAX_TERMINAL_ROWS)
        .limit(1)
        .into_tuple::<i64>()
        .one(connection)
        .await?
    {
        outcome.allocations_deleted += allocation::Entity::delete_many()
            .filter(terminal)
            .filter(allocation::Column::UpdatedAtUnix.lt(cutoff))
            .exec(connection)
            .await?
            .rows_affected;
    }
    Ok(outcome)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
