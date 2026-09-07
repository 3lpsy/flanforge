use uuid::Uuid;

use crate::manifest::OwnershipManifest;
use flanforge_libvirt_wire::Artifact;

use super::{
    DomainSpec, allocation_metadata, domain_xml, ensure_metadata_matches, guest_address,
    overlay_xml, seed_xml, warm_xml,
};

fn manifest() -> OwnershipManifest {
    let mut overlay = Artifact::new("ci-owned.root.qcow2".to_owned());
    overlay.set_key("/pool/overlay&owned".to_owned());
    let mut seed = Artifact::new("ci-owned.seed.img".to_owned());
    seed.set_key("/pool/seed-owned".to_owned());
    OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "ci-owned".to_owned(),
        Uuid::new_v4(),
        "52:54:00:01:02:03".to_owned(),
        overlay,
        seed,
        "flanforge-owned".to_owned(),
        "/var/lib/flanforge/libvirt/owned.known-hosts".into(),
        1,
    )
}

#[test]
fn domain_xml_escapes_owned_values_and_carries_exact_metadata() {
    let manifest = manifest();
    let xml = domain_xml(DomainSpec {
        manifest: &manifest,
        pool: "flanforge",
        network: "flanforge-ci",
        cpu_count: 4,
        memory_mb: 8_192,
    })
    .unwrap_or_else(|error| unreachable!("xml: {error}"));
    assert!(xml.contains("/pool/overlay&amp;owned"));
    assert!(xml.contains("org.qemu.guest_agent.0"));
    let metadata =
        allocation_metadata(&manifest).unwrap_or_else(|error| unreachable!("metadata: {error}"));
    ensure_metadata_matches(&metadata, &manifest)
        .unwrap_or_else(|error| unreachable!("match: {error}"));
    // virDomainGetMetadata strips the namespace its URI query selected, so
    // the live form arrives without the declaration and prefix.
    let stripped = metadata
        .replace("flanforge:allocation", "allocation")
        .replace(" xmlns:flanforge=\"urn:flanforge:allocation:v1\"", "");
    ensure_metadata_matches(&stripped, &manifest)
        .unwrap_or_else(|error| unreachable!("stripped match: {error}"));
    assert!(
        ensure_metadata_matches(
            &metadata.replace("urn:flanforge:allocation:v1", "urn:other:v1"),
            &manifest
        )
        .is_err()
    );
    assert!(
        ensure_metadata_matches(&metadata.replace("schema=\"1\"", "schema=\"2\""), &manifest)
            .is_err()
    );
}

#[test]
fn qga_address_requires_exact_mac_and_unambiguous_ipv4() {
    let response = r#"{"return":[{"name":"eth0","hardware-address":"52:54:00:01:02:03","ip-addresses":[{"ip-address":"192.0.2.10","ip-address-type":"ipv4","prefix":24}]}]}"#;
    assert_eq!(
        guest_address(response, "52:54:00:01:02:03")
            .unwrap_or_else(|error| unreachable!("address: {error}"))
            .map(|address| address.to_string()),
        Some("192.0.2.10".to_owned())
    );
    assert_eq!(
        guest_address(response, "52:54:00:ff:ff:ff")
            .unwrap_or_else(|error| unreachable!("address: {error}")),
        None
    );
    let ambiguous = response.replace("]}]}", ",{".to_owned().as_str());
    assert!(guest_address(&ambiguous, "52:54:00:01:02:03").is_err());
}

/// A warm image is a chain root: the flatten `virStorageVolCreateXMLFrom`
/// performs is exactly the absence of a `<backingStore>` in this XML, and its
/// format has to stay qcow2 for the overlays that will be created over it.
#[test]
fn a_warm_volume_is_declared_as_a_qcow2_chain_root() {
    let xml = warm_xml("warm-&-1.qcow2", 42 * 1_024 * 1_024 * 1_024);
    assert!(xml.contains("<format type=\"qcow2\"/>"));
    assert!(
        !xml.contains("backingStore"),
        "a warm image declares no backing file: {xml}"
    );
    assert!(xml.contains("<capacity unit=\"B\">45097156608</capacity>"));
    assert!(xml.contains("<name>warm-&amp;-1.qcow2</name>"), "{xml}");
}

/// Every volume XML interpolates a name, so every one of them escapes it, and
/// each declares the format its consumer depends on.
#[test]
fn every_volume_document_escapes_its_name_and_declares_its_format() {
    let overlay = overlay_xml("ci-&.root.qcow2", 1_024, "/pool/warm-&.qcow2")
        .unwrap_or_else(|error| unreachable!("overlay: {error}"));
    let seed = seed_xml("ci-&.seed.img", 1_024);
    for xml in [&overlay, &seed, &warm_xml("warm-&.qcow2", 1_024)] {
        assert!(xml.contains("&amp;"), "unescaped name: {xml}");
        assert!(!xml.contains("<name>ci-&."), "unescaped name: {xml}");
    }
    assert!(overlay.contains("<backingStore><path>/pool/warm-&amp;.qcow2</path>"));
    assert!(overlay.contains("<format type=\"qcow2\"/>"));
    // The seed carries a host private key, so it never inherits a pool default.
    assert!(seed.contains("<format type=\"raw\"/>"));
    assert!(seed.contains("<mode>0600</mode>"));
    // A backing path libvirt could not have produced is refused outright.
    assert!(overlay_xml("ci.root.qcow2", 1_024, "/pool/warm\n.qcow2").is_err());
    assert!(overlay_xml("ci.root.qcow2", 1_024, "").is_err());
}
