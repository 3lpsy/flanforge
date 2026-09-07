use flanforge_store::{AllocationStore, HotGuestStore, WarmImageStore};
use sea_orm::DatabaseConnection;

use super::*;
use crate::convert::tests_support::{allocation, hot_guest, warm_image};
use crate::stores::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

async fn connection(directory: &std::path::Path) -> DatabaseConnection {
    flanforge_migrations::connect_and_migrate(&directory.join("t.db"))
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"))
}

async fn write_json<T: serde::Serialize>(path: &std::path::Path, record: &T) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .unwrap_or_else(|error| unreachable!("mkdir: {error}"));
    }
    tokio::fs::write(
        path,
        serde_json::to_vec(record).unwrap_or_else(|error| unreachable!("serialize: {error}")),
    )
    .await
    .unwrap_or_else(|error| unreachable!("write: {error}"));
}

#[tokio::test]
async fn import_backfills_archives_and_runs_once() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let state_dir = directory.path().join("state");
    let record = allocation();
    write_json(&state_dir.join(format!("{}.json", record.id)), &record).await;
    let image = warm_image();
    write_json(&state_dir.join("images").join("project.warm.json"), &image).await;
    let guest = hot_guest();
    write_json(&state_dir.join("hot").join("ci-hot.json"), &guest).await;
    tokio::fs::write(state_dir.join("broken.json"), b"{not json")
        .await
        .unwrap_or_else(|error| unreachable!("write: {error}"));

    let connection = connection(directory.path()).await;
    let report = import_json_state(&connection, &state_dir)
        .await
        .unwrap_or_else(|error| unreachable!("import: {error}"))
        .unwrap_or_else(|| unreachable!("first import must run"));
    assert_eq!(
        report,
        ImportReport {
            allocations: 1,
            warm_images: 1,
            hot_guests: 1,
            corrupt: 1,
        }
    );

    // Records are queryable, originals are archived — never deleted — and the
    // quarantined file stays put for forensics.
    let loaded = SqliteAllocationStore::new(connection.clone())
        .load_recent(10)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(loaded, vec![record.clone()]);
    assert_eq!(
        SqliteWarmImageStore::new(connection.clone())
            .load(&image.profile)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(image)
    );
    assert_eq!(
        SqliteHotGuestStore::new(connection.clone())
            .load(&guest.vm_name)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(guest)
    );
    assert!(
        state_dir
            .join("json-archive/allocations")
            .join(format!("{}.json", record.id))
            .is_file()
    );
    assert!(state_dir.join("broken.json.corrupt").is_file());
    assert!(!state_dir.join(format!("{}.json", record.id)).exists());

    // A second run is a no-op: the completion mark was written last.
    assert!(
        import_json_state(&connection, &state_dir)
            .await
            .unwrap_or_else(|error| unreachable!("import: {error}"))
            .is_none()
    );
}

#[tokio::test]
async fn the_database_refuses_a_foreign_state_dir() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let connection = connection(directory.path()).await;
    ensure_state_dir_identity(&connection, std::path::Path::new("/var/lib/one"))
        .await
        .unwrap_or_else(|error| unreachable!("first bind: {error}"));
    ensure_state_dir_identity(&connection, std::path::Path::new("/var/lib/one"))
        .await
        .unwrap_or_else(|error| unreachable!("rebind: {error}"));
    assert!(matches!(
        ensure_state_dir_identity(&connection, std::path::Path::new("/var/lib/two")).await,
        Err(MetaError::ForeignStateDir { .. })
    ));
}
