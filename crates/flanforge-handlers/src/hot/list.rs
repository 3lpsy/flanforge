use flanforge_wire::HotGuestView;

use crate::{WebuiServices, views};

/// Every hot guest record: lane, age, jobs served, and claim.
pub async fn handle(services: &WebuiServices) -> Vec<HotGuestView> {
    services
        .manager
        .hot_list()
        .await
        .iter()
        .map(views::hot_guest_view)
        .collect()
}
