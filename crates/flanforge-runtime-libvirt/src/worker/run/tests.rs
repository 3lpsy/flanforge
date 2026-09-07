use std::{
    fs::{DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
};

use flanforge_libvirt_wire::{Artifact, OwnershipManifest};
use flanforge_manager::WorkerError;
use uuid::Uuid;

use crate::manifest::{allocation_dir, cleanup_path, path, save};

use super::{GuestCleanup, finish_cleanup};

#[tokio::test]
async fn cleanup_authority_survives_runner_failure_and_terminal_commit_window() {
    let state = tempfile::tempdir().unwrap_or_else(|error| unreachable!("state: {error}"));
    let allocation_id = Uuid::new_v4();
    let directory = allocation_dir(state.path(), allocation_id);
    let mut builder = DirBuilder::new();
    builder.mode(0o700).recursive(true);
    builder
        .create(&directory)
        .unwrap_or_else(|error| unreachable!("directory: {error}"));
    let artifact_id = Uuid::new_v4();
    let manifest = OwnershipManifest::new(
        allocation_id,
        Uuid::new_v4(),
        "ci-libvirt-test".to_owned(),
        Uuid::new_v4(),
        "02:01:02:03:04:05".to_owned(),
        Artifact::new(format!("root-{artifact_id}.qcow2")),
        Artifact::new(format!("seed-{artifact_id}.img")),
        "flanforge-test".to_owned(),
        directory.join("known_hosts"),
        1,
    );
    save(&manifest, &path(state.path(), allocation_id))
        .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let mut known_hosts = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("known_hosts"))
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));
    known_hosts
        .write_all(b"flanforge-test ssh-ed25519 AAAA\n")
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));
    known_hosts
        .sync_all()
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));

    assert!(
        finish_cleanup(
            state.path(),
            allocation_id,
            Ok(GuestCleanup::Owned(Box::new(manifest.clone()))),
            Err(WorkerError::new("injected runner deletion failure")),
        )
        .await
        .is_err()
    );
    assert!(path(state.path(), allocation_id).is_file());
    assert!(!cleanup_path(state.path(), allocation_id).exists());

    finish_cleanup(
        state.path(),
        allocation_id,
        Ok(GuestCleanup::Owned(Box::new(manifest))),
        Ok(()),
    )
    .await
    .unwrap_or_else(|error| unreachable!("retry: {error}"));
    assert!(!path(state.path(), allocation_id).exists());
    assert!(!directory.join("known_hosts").exists());
    assert!(cleanup_path(state.path(), allocation_id).is_file());

    finish_cleanup(
        state.path(),
        allocation_id,
        Ok(GuestCleanup::Tombstoned),
        Err(WorkerError::new(
            "Forgejo unavailable after completed cleanup",
        )),
    )
    .await
    .unwrap_or_else(|error| unreachable!("tombstone retry: {error}"));
    assert!(cleanup_path(state.path(), allocation_id).is_file());
}
