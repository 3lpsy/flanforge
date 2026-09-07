use flanforge_core::{Allocation, AllocationMode, AllocationState, VmName};
use flanforge_orm::SqliteAllocationStore;
use flanforge_store::AllocationStore;

use super::*;
use crate::{WebuiFault, tests_support};

fn record(index: u64, state: AllocationState) -> Allocation {
    let mut allocation = Allocation::new(
        flanforge_test_support::request(),
        VmName::new(format!("ci-{index}")).unwrap_or_else(|error| unreachable!("{error}")),
        flanforge_test_support::profile().runner_label,
        AllocationMode::Cold,
        flanforge_test_support::size(),
    );
    allocation.state = state;
    allocation.updated_at_unix = 1_000 + index;
    allocation
}

#[tokio::test]
async fn history_pages_newest_first_with_a_stable_cursor() {
    let (services, connection, _directory) = tests_support::services_with_connection(|_| {}).await;
    let store = SqliteAllocationStore::new(connection);
    for index in 0..5 {
        store
            .save(&record(index, AllocationState::Completed))
            .await
            .unwrap_or_else(|error| unreachable!("save: {error}"));
    }

    let first = history(&services, None, None, Some(2), None)
        .await
        .unwrap_or_else(|error| unreachable!("history: {error}"));
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[0].updated_at_unix, 1_004);
    let cursor = first
        .next_cursor
        .unwrap_or_else(|| unreachable!("more pages exist"));

    let second = history(&services, None, None, Some(2), Some(&cursor))
        .await
        .unwrap_or_else(|error| unreachable!("history: {error}"));
    assert_eq!(second.items[0].updated_at_unix, 1_002);

    // Filters narrow by token; an unknown state matches nothing.
    let none = history(&services, None, Some("running"), Some(10), None)
        .await
        .unwrap_or_else(|error| unreachable!("history: {error}"));
    assert!(none.items.is_empty());
    assert!(matches!(
        history(&services, None, None, None, Some("not a cursor")).await,
        Err(WebuiFault::Invalid(_))
    ));
}

#[tokio::test]
async fn detail_joins_the_record_with_its_events() {
    let (services, connection, _directory) = tests_support::services_with_connection(|_| {}).await;
    let store = SqliteAllocationStore::new(connection.clone());
    let allocation = record(1, AllocationState::Completed);
    store
        .save(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));
    let sink = flanforge_orm::SqliteEventSink::new(connection);
    flanforge_store::EventSink::record(
        &sink,
        flanforge_store::Event::new(flanforge_store::EventKind::AllocationStateChanged)
            .with_allocation(allocation.id),
    )
    .await;

    let detail = detail(&services, &allocation.id.to_string())
        .await
        .unwrap_or_else(|error| unreachable!("detail: {error}"));
    assert_eq!(detail.record.id, allocation.id.to_string());
    assert!(detail.live.is_none(), "terminal records are not resident");
    assert_eq!(detail.events.len(), 1);
    assert_eq!(detail.events[0].kind, "allocation_state_changed");

    assert!(matches!(
        detail_of_unknown(&services).await,
        Err(WebuiFault::NotFound)
    ));
}

async fn detail_of_unknown(
    services: &crate::WebuiServices,
) -> Result<flanforge_wire::AllocationDetail, WebuiFault> {
    detail(services, &uuid::Uuid::new_v4().to_string()).await
}
