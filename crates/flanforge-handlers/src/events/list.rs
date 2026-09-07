use flanforge_wire::EventPage;

use crate::{WebuiFault, WebuiServices, views};

/// One page of the durable "what has happened" log, newest first.
///
/// # Errors
///
/// Returns `Internal` on a store failure.
pub async fn handle(
    services: &WebuiServices,
    kind: Option<&str>,
    limit: Option<u64>,
    before: Option<i64>,
) -> Result<EventPage, WebuiFault> {
    let (rows, next) = services
        .history
        .events_page(kind, None, limit, before)
        .await?;
    Ok(EventPage {
        items: rows.iter().map(views::event_view).collect(),
        next_cursor: next,
    })
}
