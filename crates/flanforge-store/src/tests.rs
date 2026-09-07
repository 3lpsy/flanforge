use tempfile::TempDir;

use super::*;

#[tokio::test]
async fn the_mutation_lock_admits_one_holder_per_state_dir() {
    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let holder = StateMutationLock::acquire(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(holder.is_for(temporary.path()));
    assert!(!holder.is_for(&temporary.path().join("other")));
    assert!(matches!(
        StateMutationLock::acquire(temporary.path()).await,
        Err(StoreError::Lock { .. })
    ));
    drop(holder);

    let successor = StateMutationLock::acquire(temporary.path())
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    drop(successor);
}

#[cfg(unix)]
#[tokio::test]
async fn acquiring_the_lock_makes_the_state_dir_private() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().unwrap_or_else(|error| unreachable!("{error}"));
    let state_dir = temporary.path().join("state");
    let _holder = StateMutationLock::acquire(&state_dir)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    let mode = std::fs::metadata(&state_dir)
        .unwrap_or_else(|error| unreachable!("{error}"))
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    assert_eq!(
        std::fs::metadata(state_dir.join("instance.lock"))
            .unwrap_or_else(|error| unreachable!("{error}"))
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
