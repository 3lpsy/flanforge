use flanforge_orm::AllocationCursor;
use flanforge_wire::AllocationPage;

use crate::{WebuiFault, WebuiServices, views};

/// One page of durable allocation history, newest first.
///
/// # Errors
///
/// Returns `Invalid` for an unparseable cursor or `Internal` on a store
/// failure.
pub async fn handle(
    services: &WebuiServices,
    profile: Option<&str>,
    state: Option<&str>,
    limit: Option<u64>,
    before: Option<&str>,
) -> Result<AllocationPage, WebuiFault> {
    let cursor = before
        .map(|value| AllocationCursor::parse(value).ok_or(WebuiFault::Invalid("invalid cursor")))
        .transpose()?;
    let (rows, next) = services
        .history
        .allocations_page(profile, state, limit, cursor.as_ref())
        .await?;
    Ok(AllocationPage {
        items: rows.iter().map(views::allocation_record_view).collect(),
        next_cursor: next.as_ref().map(AllocationCursor::encode),
    })
}
