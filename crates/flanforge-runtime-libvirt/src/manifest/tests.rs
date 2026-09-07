use flanforge_core::{Allocation, AllocationMode, RunnerLabel, VmName};
use flanforge_test_support::{request, size};

use super::{
    BaseImageManifest, ServiceInstance, allocation_dir, ensure_matches, find_by_domain, intent,
    load, mac_for, path, save,
};
use uuid::Uuid;

use crate::RuntimeError;

#[test]
fn only_not_found_is_classified_as_an_absent_manifest() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let missing = directory.path().join("missing.json");
    assert_eq!(load(&missing), Err(RuntimeError::ManifestNotFound));

    let corrupt = directory.path().join("corrupt.json");
    std::fs::write(&corrupt, b"not json").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(matches!(load(&corrupt), Err(RuntimeError::Manifest { .. })));
}

#[test]
fn base_manifest_accepts_only_the_built_guest_contract() {
    let source = include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json");
    let parsed =
        BaseImageManifest::parse(source).unwrap_or_else(|error| unreachable!("manifest: {error}"));
    assert_eq!(parsed.image_file(), "flanforge-base.qcow2");

    let hostile =
        String::from_utf8_lossy(source).replace("flanforge-base.qcow2", "../foreign.qcow2");
    assert!(BaseImageManifest::parse(hostile.as_bytes()).is_err());
}

#[test]
fn allocation_mac_is_deterministic_local_unicast_with_forty_identity_bits() {
    let id = Uuid::parse_str("019d0000-0000-7000-8000-010203040506")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mac = mac_for(id);
    assert_eq!(mac, "02:02:03:04:05:06");
    assert_eq!(mac_for(id), mac);
    let first =
        u8::from_str_radix(&mac[..2], 16).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_eq!(first & 0b10, 0b10, "locally administered bit");
    assert_eq!(first & 0b1, 0, "unicast bit");
}

#[test]
fn ownership_round_trip_requires_allocation_and_instance_agreement() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let allocation = Allocation::new(
        request(),
        VmName::new("ci-libvirt-test").unwrap_or_else(|error| unreachable!("vm: {error}")),
        RunnerLabel::new("linux-libvirt-test")
            .unwrap_or_else(|error| unreachable!("label: {error}")),
        AllocationMode::Cold,
        size(),
    );
    let instance = ServiceInstance::load_or_create(directory.path())
        .unwrap_or_else(|error| unreachable!("instance: {error}"));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(allocation_dir(directory.path(), allocation.id.into_uuid()))
        .unwrap_or_else(|error| unreachable!("dir: {error}"));
    let path = path(directory.path(), allocation.id.into_uuid());
    let manifest = intent(
        &allocation,
        &instance,
        directory.path(),
        format!("flanforge-{}", allocation.id),
    )
    .unwrap_or_else(|error| unreachable!("intent: {error}"));
    save(&manifest, &path).unwrap_or_else(|error| unreachable!("save: {error}"));
    let loaded = load(&path).unwrap_or_else(|error| unreachable!("load: {error}"));
    ensure_matches(
        &loaded,
        allocation.id.into_uuid(),
        &allocation.vm_name,
        &instance,
        directory.path(),
    )
    .unwrap_or_else(|error| unreachable!("match: {error}"));
}

/// RUN-102: the ownership manifest outlives the allocation record, so it is
/// what authorizes collecting an orphan whose record aged out. A domain no
/// manifest claims authorizes nothing, whatever its name looks like.
#[test]
fn an_orphan_domain_resolves_only_to_the_manifest_that_claims_it() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let allocation = Allocation::new(
        request(),
        VmName::new("ci-libvirt-orphan").unwrap_or_else(|error| unreachable!("vm: {error}")),
        RunnerLabel::new("linux-libvirt-test")
            .unwrap_or_else(|error| unreachable!("label: {error}")),
        AllocationMode::Cold,
        size(),
    );
    let instance = ServiceInstance::load_or_create(directory.path())
        .unwrap_or_else(|error| unreachable!("instance: {error}"));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(allocation_dir(directory.path(), allocation.id.into_uuid()))
        .unwrap_or_else(|error| unreachable!("dir: {error}"));
    let manifest = intent(
        &allocation,
        &instance,
        directory.path(),
        format!("flanforge-{}", allocation.id),
    )
    .unwrap_or_else(|error| unreachable!("intent: {error}"));
    save(
        &manifest,
        &path(directory.path(), allocation.id.into_uuid()),
    )
    .unwrap_or_else(|error| unreachable!("save: {error}"));

    assert_eq!(
        find_by_domain(directory.path(), "ci-libvirt-orphan"),
        Ok(allocation.id.into_uuid())
    );
    assert!(find_by_domain(directory.path(), "ci-libvirt-stranger").is_err());
}

use std::os::unix::fs::DirBuilderExt;
