use std::os::unix::fs::PermissionsExt;

use flanforge_core::VmName;
use flanforge_libvirt_wire::{BaseImageManifest, PublishedBase};

use super::verify::ensure_runtime_compatible;
use super::{load_published, publication_path, publish, publish_with_post_commit_failure};

#[test]
fn publication_is_atomic_and_binds_the_requested_logical_name() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let manifest = BaseImageManifest::parse(include_bytes!(
        "../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let publication = PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-aabbccddeeff.qcow2".to_owned(),
        "/pool/flanforge-base-aabbccddeeff.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("publication: {error}"));
    publish(directory.path(), &publication)
        .unwrap_or_else(|error| unreachable!("publish: {error}"));
    let logical =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("logical: {error}"));
    assert_eq!(
        load_published(directory.path(), &logical)
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        publication
    );
    let mode = std::fs::metadata(publication_path(directory.path(), &logical))
        .unwrap_or_else(|error| unreachable!("metadata: {error}"))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    publish(directory.path(), &publication)
        .unwrap_or_else(|error| unreachable!("idempotent publish: {error}"));

    let replacement = PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-replacement.qcow2".to_owned(),
        "/pool/flanforge-base-replacement.qcow2".to_owned(),
        publication.manifest().clone(),
    )
    .unwrap_or_else(|error| unreachable!("replacement: {error}"));
    assert!(publish(directory.path(), &replacement).is_err());
    assert_eq!(
        load_published(directory.path(), &logical)
            .unwrap_or_else(|error| unreachable!("preserved: {error}")),
        publication
    );
}

#[test]
fn post_commit_failure_preserves_the_published_volume_authority() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let manifest = BaseImageManifest::parse(include_bytes!(
        "../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let publication = PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-aabbccddeeff.qcow2".to_owned(),
        "/pool/flanforge-base-aabbccddeeff.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("publication: {error}"));

    let result = publish_with_post_commit_failure(directory.path(), &publication);
    let Err(error) = result else {
        unreachable!("forced finalization must fail")
    };
    assert!(error.is_committed());
    let logical =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("logical: {error}"));
    assert_eq!(
        load_published(directory.path(), &logical)
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        publication
    );
}

#[test]
fn runtime_policy_rejects_a_structurally_valid_unsupported_guest() {
    let source = include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(source).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    document["guest"]["os"]["architecture"] = "aarch64".into();
    let bytes =
        serde_json::to_vec(&document).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manifest = BaseImageManifest::parse(&bytes)
        .unwrap_or_else(|error| unreachable!("structural wire: {error}"));
    assert!(ensure_runtime_compatible(&manifest).is_err());
}

#[test]
/// The runtime refuses only what it genuinely cannot boot: a non-qcow2 base and
/// a non-x86_64 guest. Distribution, container runtime, and guest size are the
/// operator's, so an image that differs there is still compatible.
fn runtime_refuses_only_what_it_cannot_boot() {
    let source = include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json");
    let document: serde_json::Value =
        serde_json::from_slice(source).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let parse = |value: &serde_json::Value| {
        let bytes =
            serde_json::to_vec(value).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        BaseImageManifest::parse(&bytes)
            .unwrap_or_else(|error| unreachable!("structural wire: {error}"))
    };

    let mut wrong_suffix = document.clone();
    wrong_suffix["image"]["file"] = "flanforge-base.img".into();
    assert!(ensure_runtime_compatible(&parse(&wrong_suffix)).is_err());

    let mut wrong_architecture = document.clone();
    wrong_architecture["guest"]["os"]["architecture"] = "aarch64".into();
    assert!(ensure_runtime_compatible(&parse(&wrong_architecture)).is_err());

    let mut smaller = document.clone();
    smaller["image"]["virtual_bytes"] = (8 * 1_024_u64.pow(3)).into();
    assert!(ensure_runtime_compatible(&parse(&smaller)).is_ok());

    let mut debian = document.clone();
    debian["guest"]["os"]["id"] = "debian".into();
    assert!(ensure_runtime_compatible(&parse(&debian)).is_ok());

    let mut no_container_runtime = document;
    no_container_runtime["guest"]["podman"]["rootless"] = false.into();
    no_container_runtime["guest"]["podman"]["docker_api"] = false.into();
    assert!(ensure_runtime_compatible(&parse(&no_container_runtime)).is_ok());
}
