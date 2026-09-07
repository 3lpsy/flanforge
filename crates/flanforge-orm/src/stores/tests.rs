use flanforge_core::{AllocationState, HotState};
use flanforge_store::{
    AllocationStore, Event, EventKind, EventSink, HotGuestStore, WarmImageStore,
};
use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait, PaginatorTrait};

use super::*;
use crate::convert::tests_support::{allocation, hot_guest, warm_image};
use crate::entities::event;

async fn connection() -> (tempfile::TempDir, DatabaseConnection) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("t.db"))
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    (directory, connection)
}

#[test]
fn terminal_tokens_track_the_state_machine() {
    assert!(super::allocations::terminal_tokens_match_the_state_machine());
}

#[tokio::test]
async fn allocations_upsert_and_window_like_the_json_store() {
    let (_dir, connection) = connection().await;
    let store = SqliteAllocationStore::new(connection);

    let mut live = allocation();
    store
        .save(&live)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));
    // Updating the same record must not create a second row.
    live.transition(AllocationState::Preparing)
        .unwrap_or_else(|error| unreachable!("transition: {error}"));
    store
        .save(&live)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));

    let mut old_terminal = allocation();
    old_terminal.state = AllocationState::Completed;
    old_terminal.updated_at_unix = 1_000;
    store
        .save(&old_terminal)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));
    let mut new_terminal = allocation();
    new_terminal.state = AllocationState::Failed;
    // Strictly newest, so the one-row window selects it deterministically.
    new_terminal.updated_at_unix = live.updated_at_unix + 10;
    store
        .save(&new_terminal)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));

    // The bound applies to finished history only; the live record always loads.
    let loaded = store
        .load_recent(1)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().any(|record| record.id == live.id));
    assert!(loaded.iter().any(|record| record.id == new_terminal.id));

    let loaded = store
        .load_recent(10)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(loaded.len(), 3);
}

#[tokio::test]
async fn hot_guests_round_trip_and_remove() {
    let (_dir, connection) = connection().await;
    let store = SqliteHotGuestStore::new(connection);
    let mut guest = hot_guest();
    store
        .save(&guest)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));
    guest
        .ensure_state(HotState::Idle)
        .unwrap_or_else(|error| unreachable!("state: {error}"));
    store
        .save(&guest)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));

    let loaded = store
        .load(&guest.vm_name)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(loaded.as_ref(), Some(&guest));
    assert_eq!(
        store
            .load_all()
            .await
            .unwrap_or_else(|error| unreachable!("load_all: {error}"))
            .len(),
        1
    );

    store
        .remove(&guest.vm_name)
        .await
        .unwrap_or_else(|error| unreachable!("remove: {error}"));
    assert!(
        store
            .load(&guest.vm_name)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}"))
            .is_none()
    );
}

#[tokio::test]
async fn warm_images_round_trip_and_remove() {
    let (_dir, connection) = connection().await;
    let store = SqliteWarmImageStore::new(connection);
    let record = warm_image();
    store
        .save(&record)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));
    assert_eq!(
        store
            .load(&record.profile)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}"))
            .as_ref(),
        Some(&record)
    );
    store
        .remove(&record.profile)
        .await
        .unwrap_or_else(|error| unreachable!("remove: {error}"));
    assert!(
        store
            .load_all()
            .await
            .unwrap_or_else(|error| unreachable!("load_all: {error}"))
            .is_empty()
    );
}

#[tokio::test]
async fn events_append_and_age_out() {
    let (_dir, connection) = connection().await;
    let sink = SqliteEventSink::new(connection.clone());
    sink.record(
        Event::new(EventKind::AllocationStateChanged)
            .with_allocation(allocation().id)
            .with_payload(&serde_json::json!({"from": "requested", "to": "preparing"})),
    )
    .await;

    // A row far older than the retention window is deleted by a prune pass.
    let stale = event::ActiveModel {
        id: sea_orm::ActiveValue::NotSet,
        occurred_at_unix: Set(1_000),
        kind: Set("config_reloaded".to_owned()),
        allocation_id: Set(None),
        vm_name: Set(None),
        profile: Set(None),
        actor: Set(None),
        payload: Set(None),
    };
    event::Entity::insert(stale)
        .exec(&connection)
        .await
        .unwrap_or_else(|error| unreachable!("insert: {error}"));

    let outcome = prune_history(&connection)
        .await
        .unwrap_or_else(|error| unreachable!("prune: {error}"));
    assert_eq!(outcome.events_deleted, 1);
    assert_eq!(
        event::Entity::find()
            .count(&connection)
            .await
            .unwrap_or_else(|error| unreachable!("count: {error}")),
        1
    );
}
