use flanforge_wire::AllocationDetail;

use crate::{WebuiFault, WebuiServices, views};

const DETAIL_EVENT_LIMIT: u64 = 100;

/// One allocation: its durable record, its live projection while resident,
/// and its event timeline, newest first.
///
/// # Errors
///
/// Returns `NotFound` for an unknown id or `Internal` on a store failure.
pub async fn handle(services: &WebuiServices, id: &str) -> Result<AllocationDetail, WebuiFault> {
    let record = services
        .history
        .allocation_by_id(id)
        .await?
        .ok_or(WebuiFault::NotFound)?;
    let live = services
        .manager
        .list()
        .await
        .iter()
        .find(|summary| summary.id.to_string() == record.id)
        .map(views::allocation_view);
    let (events, _) = services
        .history
        .events_page(None, Some(id), Some(DETAIL_EVENT_LIMIT), None)
        .await?;
    Ok(AllocationDetail {
        record: views::allocation_record_view(&record),
        live,
        events: events.iter().map(views::event_view).collect(),
    })
}
