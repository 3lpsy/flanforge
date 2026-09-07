use flanforge_libvirt_wire::{
    Artifact, CheckpointVolumeRole, HelperConfig, OwnershipManifest, VolumeCheckpoint,
};
use std::os::unix::fs::PermissionsExt;
use uuid::Uuid;

use super::{VolumeJournal, allocation_path, import_key, load, merge_allocation, remove, save};

#[test]
fn checkpoint_round_trip_is_private_atomic_and_removable() {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let instance = Uuid::new_v4();
    let allocation = Uuid::new_v4();
    let artifact = Uuid::new_v4();
    let directory = flanforge_paths::libvirt_state_paths(root.path())
        .allocations
        .join(allocation.to_string());
    std::fs::create_dir_all(&directory).unwrap_or_else(|error| unreachable!("directory: {error}"));
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("permissions: {error}"));
    let mut checkpoint = VolumeCheckpoint::allocation(instance, "flanforge".to_owned(), allocation)
        .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    let path = allocation_path(root.path(), allocation);
    save(&checkpoint, &path).unwrap_or_else(|error| unreachable!("save intent: {error}"));
    checkpoint
        .record(
            CheckpointVolumeRole::Overlay,
            format!("root-{artifact}.qcow2"),
            "/pool/root.qcow2".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("record: {error}"));
    save(&checkpoint, &path).unwrap_or_else(|error| unreachable!("save volume: {error}"));

    assert_eq!(load(&path), Ok(Some(checkpoint)));
    remove(&path).unwrap_or_else(|error| unreachable!("remove: {error}"));
    assert_eq!(load(&path), Ok(None));
}

#[test]
fn durable_helper_checkpoint_recovers_keys_after_the_reply_is_lost() {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let instance = Uuid::new_v4();
    let allocation = Uuid::new_v4();
    let artifact = Uuid::new_v4();
    let directory = flanforge_paths::libvirt_state_paths(root.path())
        .allocations
        .join(allocation.to_string());
    std::fs::create_dir_all(&directory).unwrap_or_else(|error| unreachable!("directory: {error}"));
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("permissions: {error}"));
    let config = HelperConfig {
        uri: "qemu:///system".to_owned(),
        pool: "flanforge".to_owned(),
        network: "flanforge-ci".to_owned(),
        state_dir: root.path().to_path_buf(),
        service_instance: instance,
        min_storage_free_bytes: 1,
        allow_insecure_transport: false,
        is_warm_declared: false,
    };
    let mut manifest = OwnershipManifest::new(
        allocation,
        instance,
        "flanforge-allocation".to_owned(),
        Uuid::new_v4(),
        "02:00:00:00:00:01".to_owned(),
        Artifact::new(format!("root-{artifact}.qcow2")),
        Artifact::new(format!("seed-{artifact}.img")),
        "flanforge-allocation".to_owned(),
        directory.join("known_hosts"),
        1,
    );
    let mut journal = VolumeJournal::allocation(&config, &manifest)
        .unwrap_or_else(|error| unreachable!("journal: {error}"));
    journal
        .record(
            CheckpointVolumeRole::Overlay,
            manifest.overlay().name().to_owned(),
            "/pool/root.qcow2".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("overlay: {error}"));
    journal
        .record(
            CheckpointVolumeRole::Seed,
            manifest.seed().name().to_owned(),
            "/pool/seed.img".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("seed: {error}"));
    drop(journal);

    let checkpoint = load(&allocation_path(root.path(), allocation))
        .unwrap_or_else(|error| unreachable!("load: {error}"))
        .unwrap_or_else(|| unreachable!("checkpoint"));
    merge_allocation(&checkpoint, &mut manifest, instance, "flanforge")
        .unwrap_or_else(|error| unreachable!("recover: {error}"));
    assert_eq!(manifest.overlay().key(), Some("/pool/root.qcow2"));
    assert_eq!(manifest.seed().key(), Some("/pool/seed.img"));
}

#[test]
fn empty_import_checkpoint_is_not_ownership_evidence() {
    let instance = Uuid::new_v4();
    let import = Uuid::new_v4();
    let name = format!("base-import-{import}.qcow2");
    let checkpoint = VolumeCheckpoint::image_import(instance, "flanforge".to_owned(), import)
        .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    assert!(import_key(&checkpoint, instance, "flanforge", import, &name).is_err());
}

#[test]
fn foreign_checkpoint_never_mutates_allocation_authority() {
    let instance = Uuid::new_v4();
    let allocation = Uuid::new_v4();
    let artifact = Uuid::new_v4();
    let overlay = format!("root-{artifact}.qcow2");
    let seed = format!("seed-{artifact}.img");
    let mut manifest = OwnershipManifest::new(
        allocation,
        instance,
        "flanforge-allocation".to_owned(),
        Uuid::new_v4(),
        "02:00:00:00:00:01".to_owned(),
        Artifact::new(overlay.clone()),
        Artifact::new(seed),
        "flanforge-allocation".to_owned(),
        "/var/lib/flanforge/known_hosts".into(),
        1,
    );
    let mut checkpoint = VolumeCheckpoint::allocation(instance, "foreign".to_owned(), allocation)
        .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    checkpoint
        .record(
            CheckpointVolumeRole::Overlay,
            overlay,
            "/foreign/root.qcow2".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("record: {error}"));

    assert!(merge_allocation(&checkpoint, &mut manifest, instance, "flanforge").is_err());
    assert_eq!(manifest.overlay().key(), None);
}
