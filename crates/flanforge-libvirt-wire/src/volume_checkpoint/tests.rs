use uuid::Uuid;

use super::{CheckpointVolumeRole, VolumeCheckpoint};

#[test]
fn allocation_checkpoint_binds_both_artifacts_to_one_operation() {
    let instance = Uuid::new_v4();
    let allocation = Uuid::new_v4();
    let artifact = Uuid::new_v4();
    let overlay = format!("root-{artifact}.qcow2");
    let seed = format!("seed-{artifact}.img");
    let mut checkpoint = VolumeCheckpoint::allocation(instance, "flanforge".to_owned(), allocation)
        .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    checkpoint
        .record(
            CheckpointVolumeRole::Overlay,
            overlay.clone(),
            "/pool/root.qcow2".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("overlay: {error}"));
    checkpoint
        .record(
            CheckpointVolumeRole::Seed,
            seed.clone(),
            "/pool/seed.img".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("seed: {error}"));

    let encoded = checkpoint
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let decoded =
        VolumeCheckpoint::parse(&encoded).unwrap_or_else(|error| unreachable!("parse: {error}"));
    assert!(
        decoded
            .ensure_allocation(instance, "flanforge", allocation, &overlay, &seed)
            .is_ok()
    );
    assert!(
        decoded
            .ensure_allocation(Uuid::new_v4(), "flanforge", allocation, &overlay, &seed)
            .is_err()
    );
}

#[test]
fn import_checkpoint_rejects_a_different_generated_name_or_key_traversal() {
    let instance = Uuid::new_v4();
    let import = Uuid::new_v4();
    let name = format!("base-import-{import}.qcow2");
    let mut checkpoint = VolumeCheckpoint::image_import(instance, "flanforge".to_owned(), import)
        .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    for key in [
        "../foreign",
        "/pool/../foreign",
        "/pool//foreign",
        "/pool/./foreign",
    ] {
        assert!(
            checkpoint
                .record(CheckpointVolumeRole::Base, name.clone(), key.to_owned())
                .is_err(),
            "{key}"
        );
    }
    checkpoint
        .record(
            CheckpointVolumeRole::Base,
            name.clone(),
            "/pool/base.qcow2".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("base: {error}"));
    assert!(
        checkpoint
            .ensure_import(instance, "flanforge", import, &name)
            .is_ok()
    );
    assert!(
        checkpoint
            .ensure_import(
                instance,
                "flanforge",
                import,
                &format!("base-import-{}.qcow2", Uuid::new_v4()),
            )
            .is_err()
    );
}
