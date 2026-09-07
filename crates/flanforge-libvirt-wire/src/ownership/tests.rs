use std::path::PathBuf;

use uuid::Uuid;

use super::{
    Artifact, CleanupTombstone, DomainOwnershipMetadata, LIBVIRT_OWNERSHIP_METADATA_URI,
    OwnershipManifest,
};

fn manifest(known_hosts_file: PathBuf) -> OwnershipManifest {
    OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "ci-project-1-1".to_owned(),
        Uuid::new_v4(),
        "52:54:00:00:00:01".to_owned(),
        Artifact::new("ci-project-1-1.root.qcow2".to_owned()),
        Artifact::new("ci-project-1-1.seed.img".to_owned()),
        "flanforge-allocation".to_owned(),
        known_hosts_file,
        1,
    )
}

#[test]
fn ownership_manifest_rejects_relative_or_non_normal_host_key_anchors() {
    for path in [
        PathBuf::from("known_hosts"),
        PathBuf::from("/var/lib/flanforge/./known_hosts"),
        PathBuf::from("/var/lib/flanforge//known_hosts"),
    ] {
        assert!(manifest(path).ensure_valid().is_err());
    }
}

#[test]
fn domain_metadata_requires_the_complete_bounded_v1_shape() {
    let allocation = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let domain = Uuid::new_v4();
    let complete = format!(
        "<flanforge:allocation xmlns:flanforge=\"{LIBVIRT_OWNERSHIP_METADATA_URI}\" schema=\"1\" allocation=\"{allocation}\" instance=\"{instance}\" domain=\"{domain}\" overlay=\"/pool/root.qcow2\" seed=\"/pool/seed.img\"/>"
    );
    let parsed = DomainOwnershipMetadata::parse(&complete)
        .unwrap_or_else(|error| unreachable!("metadata: {error}"));
    assert_eq!(parsed.allocation_id(), allocation);
    assert_eq!(parsed.service_instance(), instance);
    assert_eq!(parsed.domain_uuid(), domain);

    for invalid in [
        complete.replace(" schema=\"1\"", ""),
        complete.replace(&allocation.to_string(), &Uuid::nil().to_string()),
        complete.replace(" seed=\"/pool/seed.img\"", ""),
        complete.replace("/pool/root.qcow2", "/pool/../foreign"),
        complete.replace("/pool/root.qcow2", "/pool/./foreign"),
        format!("{complete}{}", " ".repeat(8 * 1_024)),
    ] {
        assert!(DomainOwnershipMetadata::parse(&invalid).is_err());
    }
}

#[test]
fn cleanup_tombstone_round_trip_rejects_partial_identity() {
    let tombstone = CleanupTombstone::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "ci-project-1-1".to_owned(),
        Uuid::new_v4(),
        1,
    );
    let bytes = tombstone
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert_eq!(
        CleanupTombstone::parse(&bytes).unwrap_or_else(|error| unreachable!("parse: {error}")),
        tombstone
    );

    let invalid = CleanupTombstone::new(
        Uuid::new_v4(),
        Uuid::nil(),
        "ci-project-1-1".to_owned(),
        Uuid::new_v4(),
        1,
    );
    assert!(invalid.ensure_valid().is_err());
}

#[test]
fn ownership_manifest_rejects_nil_identity_and_path_traversal() {
    let manifest = OwnershipManifest::new(
        Uuid::nil(),
        Uuid::new_v4(),
        "ci-project-1-1".to_owned(),
        Uuid::new_v4(),
        "52:54:00:00:00:01".to_owned(),
        Artifact::new("../foreign.qcow2".to_owned()),
        Artifact::new("ci-project-1-1.seed.img".to_owned()),
        "unsafe alias".to_owned(),
        PathBuf::from("/var/lib/flanforge/../foreign/known-hosts"),
        0,
    );
    assert!(manifest.ensure_valid().is_err());
}
