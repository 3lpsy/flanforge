use std::os::unix::fs::PermissionsExt;

use super::atomic_write;

#[tokio::test]
async fn atomic_write_replaces_content_and_applies_mode() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("service.file");
    atomic_write(&path, b"first", 0o640)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    atomic_write(&path, b"second", 0o600)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));

    assert_eq!(std::fs::read(&path).unwrap_or_default(), b"second");
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|error| unreachable!("{error}"))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test]
async fn failed_publication_does_not_leave_a_temporary_file() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("missing").join("service.file");
    assert!(atomic_write(&path, b"content", 0o600).await.is_err());
    let entries = std::fs::read_dir(directory.path())
        .unwrap_or_else(|error| unreachable!("{error}"))
        .count();
    assert_eq!(entries, 0);
}
