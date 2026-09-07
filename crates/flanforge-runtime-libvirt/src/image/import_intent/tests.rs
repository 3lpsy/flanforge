use flanforge_core::VmName;
use flanforge_libvirt_wire::BaseImageManifest;
use uuid::Uuid;

use crate::{
    RuntimeError,
    image::{ImportJournal, StagedImage},
};

#[test]
fn journal_round_trip_binds_stage_and_volume_identity() {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let id = Uuid::new_v4();
    let directory = root.path().join("libvirt/imports");
    let journal =
        ImportJournal::open(root.path()).unwrap_or_else(|error| unreachable!("journal: {error}"));
    let path = directory.join(format!("import-{id}.qcow2"));
    std::fs::write(&path, b"image").unwrap_or_else(|error| unreachable!("stage: {error}"));
    let manifest = BaseImageManifest::parse(include_bytes!(
        "../../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let staged = StagedImage {
        path,
        manifest,
        volume_name: format!("base-import-{id}.qcow2"),
    };
    let intent = journal
        .record(
            VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("name: {error}")),
            &staged,
        )
        .unwrap_or_else(|error| unreachable!("record: {error}"));
    assert!(matches!(
        intent.owned_volume_key(),
        Err(RuntimeError::Ownership { .. })
    ));
    assert_eq!(journal.pending(), Ok(vec![intent.clone()]));
    let intent = journal
        .mark_created(&intent, "/pool/base-import.qcow2".to_owned())
        .unwrap_or_else(|error| unreachable!("mark: {error}"));
    assert_eq!(intent.volume_key(), Some("/pool/base-import.qcow2"));
    assert_eq!(journal.pending(), Ok(vec![intent.clone()]));
    journal
        .finish(&intent)
        .unwrap_or_else(|error| unreachable!("finish: {error}"));
    assert!(journal.pending().is_ok_and(|pending| pending.is_empty()));
    assert!(!staged.path.exists());
}

#[test]
fn journal_rejects_unsafe_volume_keys() {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let id = Uuid::new_v4();
    let directory = root.path().join("libvirt/imports");
    let journal =
        ImportJournal::open(root.path()).unwrap_or_else(|error| unreachable!("journal: {error}"));
    let path = directory.join(format!("import-{id}.qcow2"));
    std::fs::write(&path, b"image").unwrap_or_else(|error| unreachable!("stage: {error}"));
    let manifest = BaseImageManifest::parse(include_bytes!(
        "../../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let staged = StagedImage {
        path,
        manifest,
        volume_name: format!("base-import-{id}.qcow2"),
    };
    let intent = journal
        .record(
            VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("name: {error}")),
            &staged,
        )
        .unwrap_or_else(|error| unreachable!("record: {error}"));

    for key in [
        "../foreign",
        "pool/../foreign",
        "/pool//foreign",
        "/pool/./foreign",
    ] {
        assert!(
            journal.mark_created(&intent, key.to_owned()).is_err(),
            "{key}"
        );
    }
    assert!(matches!(
        journal.pending().as_deref(),
        Ok([pending]) if pending.owned_volume_key().is_err()
    ));
}

#[test]
fn journal_refuses_to_authorize_an_external_matching_stage_name() {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let external = tempfile::tempdir().unwrap_or_else(|error| unreachable!("external: {error}"));
    let id = Uuid::new_v4();
    let path = external.path().join(format!("import-{id}.qcow2"));
    std::fs::write(&path, b"do not delete")
        .unwrap_or_else(|error| unreachable!("external stage: {error}"));
    let manifest = BaseImageManifest::parse(include_bytes!(
        "../../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let staged = StagedImage {
        path: path.clone(),
        manifest,
        volume_name: format!("base-import-{id}.qcow2"),
    };
    let journal =
        ImportJournal::open(root.path()).unwrap_or_else(|error| unreachable!("journal: {error}"));
    let logical =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("name: {error}"));
    assert!(journal.record(logical, &staged).is_err());
    assert_eq!(
        std::fs::read(path).unwrap_or_else(|error| unreachable!("external read: {error}")),
        b"do not delete"
    );
}
