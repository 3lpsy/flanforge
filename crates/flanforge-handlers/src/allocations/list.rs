use flanforge_wire::AllocationView;

use crate::{WebuiServices, views};

/// The live allocations, optionally narrowed by state or profile token.
pub async fn handle(
    services: &WebuiServices,
    state: Option<&str>,
    profile: Option<&str>,
) -> Vec<AllocationView> {
    services
        .manager
        .list()
        .await
        .iter()
        .map(views::allocation_view)
        .filter(|view| state.is_none_or(|state| view.state == state))
        .filter(|view| profile.is_none_or(|profile| view.profile == profile))
        .collect()
}
