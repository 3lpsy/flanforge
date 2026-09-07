use flanforge_libvirt_wire::{
    DomainOwnershipMetadata, LIBVIRT_OWNERSHIP_METADATA_URI, MAX_DOMAIN_OWNERSHIP_METADATA_BYTES,
};
use flanforge_manager::MachineOwnership;
use uuid::Uuid;

use super::classify_metadata;

fn document(allocation: Uuid, instance: Uuid, domain: Uuid) -> String {
    format!(
        "<flanforge:allocation xmlns:flanforge=\"{LIBVIRT_OWNERSHIP_METADATA_URI}\" schema=\"1\" allocation=\"{allocation}\" instance=\"{instance}\" domain=\"{domain}\" overlay=\"/pool/root.qcow2\" seed=\"/pool/seed.img\"/>"
    )
}

/// RUN-728: only a complete document bound to the live domain and current
/// service instance enters the owned capacity path.
#[test]
fn complete_metadata_distinguishes_owned_foreign_and_unknown() {
    let allocation = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let domain = Uuid::new_v4();
    let metadata = DomainOwnershipMetadata::parse(&document(allocation, instance, domain))
        .unwrap_or_else(|error| unreachable!("metadata: {error}"));
    assert_eq!(
        classify_metadata(&metadata, domain, instance),
        MachineOwnership::Owned
    );
    assert_eq!(
        classify_metadata(&metadata, domain, Uuid::new_v4()),
        MachineOwnership::Foreign
    );
    assert_eq!(
        classify_metadata(&metadata, Uuid::new_v4(), instance),
        MachineOwnership::Unknown
    );
}

#[test]
fn partial_malformed_nil_and_oversized_metadata_are_unknown() {
    let allocation = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let domain = Uuid::new_v4();
    let complete = document(allocation, instance, domain);
    for metadata in [
        "not XML".to_owned(),
        complete.replace(" schema=\"1\"", ""),
        complete.replace(&allocation.to_string(), &Uuid::nil().to_string()),
        complete.replace(" overlay=\"/pool/root.qcow2\"", ""),
        complete.replace(" seed=\"/pool/seed.img\"", ""),
        "x".repeat(MAX_DOMAIN_OWNERSHIP_METADATA_BYTES + 1),
    ] {
        assert!(DomainOwnershipMetadata::parse(&metadata).is_err());
    }
}
