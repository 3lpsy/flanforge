use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use flanforge_core::{
    Allocation, AllocationMode, AllocationRequest, BaseFingerprint, GuestSize, ProfileName,
    RepositoryName, RunnerLabel, VmName, WarmImageRecord, WarmImageState,
};
use tempfile::TempDir;

use super::*;

fn allocation() -> Allocation {
    Allocation::new(
        AllocationRequest {
            profile: ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}")),
            repository: RepositoryName::new("owner/halogen")
                .unwrap_or_else(|error| unreachable!("{error}")),
            run_id: 10,
            run_attempt: 1,
        },
        VmName::new("ci-halogen-10-1").unwrap_or_else(|error| unreachable!("{error}")),
        RunnerLabel::new("macos-tart-halogen-allocation")
            .unwrap_or_else(|error| unreachable!("{error}")),
        AllocationMode::Cold,
        GuestSize {
            cpu_count: 4,
            memory_mb: 8_192,
        },
    )
}

#[tokio::test]
async fn state_round_trips_atomically() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let allocation = allocation();
    store
        .save(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        store
            .load_recent(1_000)
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        vec![allocation]
    );
}

#[tokio::test]
async fn state_directory_allows_only_one_daemon() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let first = Arc::new(
        JsonStateStore::open(temporary.path())
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
    );
    let second = JsonStateStore::open(temporary.path()).await;
    assert!(matches!(second, Err(StoreError::Lock { .. })));
    drop(first);
    assert!(JsonStateStore::open(temporary.path()).await.is_ok());
}

#[tokio::test]
async fn corrupt_state_is_quarantined_without_blocking_startup() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let allocation = allocation();
    store
        .save(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    tokio::fs::write(temporary.path().join("bad.json"), b"not json")
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    tokio::fs::write(
        temporary.path().join("stale.json"),
        br#"{"id":"not-a-uuid"}"#,
    )
    .await
    .unwrap_or_else(|error| unreachable!("{error}"));

    assert_eq!(
        store
            .load_recent(1_000)
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        vec![allocation]
    );
    assert!(temporary.path().join("bad.json.corrupt").exists());
    assert!(temporary.path().join("stale.json.corrupt").exists());
    assert!(!temporary.path().join("bad.json").exists());
}

#[tokio::test]
async fn structurally_invalid_state_is_not_persisted() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let mut allocation = allocation();
    allocation.request.run_id = 0;

    assert!(matches!(
        store.save(&allocation).await,
        Err(StoreError::InvalidAllocation { .. })
    ));
}

#[tokio::test]
async fn loads_only_the_most_recent_allocations_without_deleting_history() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    for _ in 0..1_001 {
        let allocation = allocation();
        let bytes = serde_json::to_vec(&allocation).unwrap_or_else(|error| unreachable!("{error}"));
        tokio::fs::write(
            temporary.path().join(format!("{}.json", allocation.id)),
            bytes,
        )
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    }

    let loaded = store
        .load_recent(1_000)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(loaded.len(), 1_000);

    let mut directory = tokio::fs::read_dir(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let mut persisted = 0;
    while let Some(entry) = directory
        .next_entry()
        .await
        .unwrap_or_else(|error| unreachable!("{error}"))
    {
        if entry
            .path()
            .extension()
            .is_some_and(|value| value == "json")
        {
            persisted += 1;
        }
    }
    assert_eq!(persisted, 1_001);
}

#[test]
fn recent_path_selection_is_newest_first_and_bounded() {
    let mut paths = vec![
        (UNIX_EPOCH + Duration::from_secs(1), "old.json".into()),
        (UNIX_EPOCH + Duration::from_secs(3), "new.json".into()),
        (UNIX_EPOCH + Duration::from_secs(2), "middle.json".into()),
    ];
    super::json::retain_recent(&mut paths, 2);
    assert_eq!(paths[0].1, std::path::PathBuf::from("new.json"));
    assert_eq!(paths[1].1, std::path::PathBuf::from("middle.json"));
}

/// A record written before ARCH-400, byte for byte.
const LEGACY_RECORD: &str = r#"{
  "id": "6f1b6b1a-9c9d-4a7b-8f0a-7d5f4b3c2a10",
  "request": {
    "profile": "halogen",
    "repository": "owner/halogen",
    "run_id": 10,
    "run_attempt": 1
  },
  "state": "requested",
  "vm_name": "ci-halogen-10-1",
  "runner_label": "macos-tart-halogen-allocation",
  "vm_created": false,
  "runner_id": null,
  "created_at_unix": 1755324251,
  "updated_at_unix": 1755324251,
  "error": null
}"#;

#[tokio::test]
async fn a_record_written_before_this_change_loads_and_validates() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    tokio::fs::write(
        temporary
            .path()
            .join("6f1b6b1a-9c9d-4a7b-8f0a-7d5f4b3c2a10.json"),
        LEGACY_RECORD,
    )
    .await
    .unwrap_or_else(|error| unreachable!("{error}"));

    let loaded = store
        .load_recent(1_000)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(loaded.len(), 1);
    let allocation = loaded.first().unwrap_or_else(|| unreachable!("record"));
    assert_eq!(allocation.mode, AllocationMode::Cold);
    assert_eq!(allocation.size, None);
    assert_eq!(allocation.source, None);
    assert_eq!(allocation.retention, None);
}

/// The shape an older binary deserializes: no new fields, no `deny_unknown_fields`.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct OldAllocation {
    id: flanforge_core::AllocationId,
    state: flanforge_core::AllocationState,
    vm_name: VmName,
}

#[tokio::test]
async fn a_record_written_after_this_change_survives_a_field_drop() {
    let mut allocation = allocation();
    allocation.mode = AllocationMode::Regenerate;
    let json = serde_json::to_string(&allocation).unwrap_or_else(|error| unreachable!("{error}"));
    assert!(json.contains("\"mode\":\"regenerate\""));

    // An older binary ignores the new fields instead of quarantining the record.
    let old = serde_json::from_str::<OldAllocation>(&json)
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(old.id, allocation.id);
}

fn warm_image_record() -> WarmImageRecord {
    WarmImageRecord {
        profile: ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}")),
        warm_template: VmName::new("halogen-warm").unwrap_or_else(|error| unreachable!("{error}")),
        generation: 3,
        base_fingerprint: BaseFingerprint::new("abcd1234")
            .unwrap_or_else(|error| unreachable!("{error}")),
        produced_by: allocation().id,
        produced_at_unix: 1_755_324_251,
        state: WarmImageState::Promoted,
        previous: None,
    }
}

#[tokio::test]
async fn a_warm_image_record_round_trips_and_an_invalid_one_is_quarantined() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonWarmImageStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let record = warm_image_record();
    store
        .save(&record)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        store
            .load(&record.profile)
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        Some(record.clone())
    );

    let images = temporary.path().join("images");
    tokio::fs::write(images.join("broken.json"), b"not json")
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        store
            .load_all()
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        vec![record.clone()]
    );
    assert!(images.join("broken.json.corrupt").exists());

    store
        .remove(&record.profile)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        store
            .load(&record.profile)
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        None
    );
}

#[tokio::test]
async fn a_structurally_invalid_warm_image_record_is_not_persisted() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonWarmImageStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let mut record = warm_image_record();
    record.generation = 0;

    assert!(matches!(
        store.save(&record).await,
        Err(StoreError::InvalidWarmImage { .. })
    ));
}

#[tokio::test]
async fn the_image_directory_is_skipped_by_the_allocation_loader() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let store = JsonStateStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let images = JsonWarmImageStore::open(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    images
        .save(&warm_image_record())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let allocation = allocation();
    store
        .save(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));

    assert_eq!(
        store
            .load_recent(1_000)
            .await
            .unwrap_or_else(|error| unreachable!("{error}")),
        vec![allocation]
    );
}
