use super::{BaseImageManifest, PublishedBase};

const TEMPLATE_FIXTURE: &[u8] =
    include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json");

fn fixture_document() -> serde_json::Value {
    serde_json::from_slice(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn encode(document: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(document).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

/// An operator who built a qcow2 without the Packer template has no
/// provenance to write, and should not have to invent any. Only `image` is
/// required; a typo in a section that is present is still an error.
#[test]
fn a_manifest_describing_only_the_image_is_accepted() {
    let bare = serde_json::json!({
        "image": {
            "file": "custom.qcow2",
            "format": "qcow2",
            "sha256": "4bd5099b11c4c1aafbbd85c9e5b18b4a891516a7386b4a7997f37983fa181b13",
            "bytes": 14,
            "virtual_bytes": 42_949_672_960_u64,
        }
    });
    let manifest = BaseImageManifest::parse(&encode(&bare))
        .unwrap_or_else(|error| unreachable!("bare manifest: {error}"));
    assert_eq!(manifest.image_file(), "custom.qcow2");
    assert_eq!(manifest.guest_architecture(), None);
    assert_eq!(manifest.guest_os_id(), None);

    // The same shape the daemon derives when no manifest is supplied at all.
    let derived = BaseImageManifest::from_image(
        "custom.qcow2".to_owned(),
        "qcow2".to_owned(),
        "4bd5099b11c4c1aafbbd85c9e5b18b4a891516a7386b4a7997f37983fa181b13".to_owned(),
        14,
        42_949_672_960,
    );
    assert_eq!(derived.ensure_valid(), Ok(()));
    assert_eq!(derived, manifest);

    let mut typo = bare;
    typo["imagee"] = serde_json::json!({});
    assert!(BaseImageManifest::parse(&encode(&typo)).is_err());
}

#[test]
fn template_golden_manifest_is_the_wire_contract() {
    let manifest = BaseImageManifest::parse(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("golden fixture: {error}"));
    assert_eq!(manifest.image_file(), "flanforge-base.qcow2");
    assert_eq!(manifest.virtual_bytes(), 42_949_672_960);
}

#[test]
fn published_base_round_trip_binds_logical_name_and_live_volume_key() {
    let manifest = BaseImageManifest::parse(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let publication = PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-aabbccddeeff.qcow2".to_owned(),
        "/var/lib/libvirt/images/flanforge-base-aabbccddeeff.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("publication: {error}"));
    let encoded = publication
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert_eq!(
        PublishedBase::parse(&encoded).unwrap_or_else(|error| unreachable!("parse: {error}")),
        publication
    );
}

#[test]
fn wire_accepts_structural_guest_capabilities_without_runtime_policy() {
    let mut document = fixture_document();
    document["guest"]["os"]["id"] = "other-linux".into();
    document["guest"]["os"]["architecture"] = "aarch64".into();
    document["guest"]["podman"]["rootless"] = false.into();
    assert!(BaseImageManifest::parse(&encode(&document)).is_ok());
}

#[test]
fn wire_leaves_image_size_and_suffix_compatibility_to_the_runtime() {
    let mut document = fixture_document();
    document["image"]["file"] = "flanforge-base.img".into();
    document["image"]["virtual_bytes"] = (8 * 1_024_u64.pow(3)).into();
    assert!(BaseImageManifest::parse(&encode(&document)).is_ok());
}

#[test]
fn dependency_proxy_flags_must_describe_one_guest_state() {
    let mut document = fixture_document();
    document["guest"]["dependency_proxy_configured"] = false.into();
    assert!(BaseImageManifest::parse(&encode(&document)).is_err());
}

#[test]
fn created_at_must_be_a_real_rfc3339_timestamp() {
    let mut document = fixture_document();
    for invalid in ["2026-02-30T03:04:05Z", "tomorrow", "2026-01-02 03:04:05"] {
        document["created_at"] = invalid.into();
        assert!(
            BaseImageManifest::parse(&encode(&document)).is_err(),
            "accepted {invalid}"
        );
    }
}

/// The record a contract-1 template wrote is durable state under `state_dir`,
/// so widening the contract must not strand a base already imported.
#[test]
fn a_contract_one_base_still_decodes_and_reports_no_agent_channel() {
    let mut document = fixture_document();
    document["guest"]["guest_contract_version"] = 1.into();
    let guest = document["guest"]
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("fixture: guest is not an object"));
    guest.remove("guest_agent");
    guest.remove("job_account");
    let manifest = BaseImageManifest::parse(&encode(&document))
        .unwrap_or_else(|error| unreachable!("contract 1: {error}"));
    assert_eq!(manifest.guest_contract_version(), Some(1));
    assert_eq!(manifest.is_guest_exec_enabled(), Some(false));
    assert_eq!(manifest.guest_job_account(), None);
}

#[test]
fn a_contract_two_base_reports_the_agent_channel_and_its_job_account() {
    let manifest = BaseImageManifest::parse(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("golden fixture: {error}"));
    assert_eq!(manifest.guest_contract_version(), Some(2));
    assert_eq!(manifest.is_guest_exec_enabled(), Some(true));
    assert_eq!(manifest.guest_job_account(), Some(("runner", 2_000)));
    assert_eq!(
        manifest.guest_privileged_account(),
        Some(("prunner", 2_001))
    );
}

/// A base with no guest object at all is a hand-built qcow2, which the runtime
/// probes rather than refuses; it is not a base reporting a blocked channel.
#[test]
fn a_manifest_without_a_guest_object_reports_no_agent_verdict() {
    let derived = BaseImageManifest::from_image(
        "custom.qcow2".to_owned(),
        "qcow2".to_owned(),
        "4bd5099b11c4c1aafbbd85c9e5b18b4a891516a7386b4a7997f37983fa181b13".to_owned(),
        14,
        42_949_672_960,
    );
    assert_eq!(derived.guest_contract_version(), None);
    assert_eq!(derived.is_guest_exec_enabled(), None);
}

#[test]
fn a_contract_two_base_may_report_its_exec_channel_blocked() {
    let mut document = fixture_document();
    document["guest"]["guest_agent"]["exec_enabled"] = false.into();
    let manifest = BaseImageManifest::parse(&encode(&document))
        .unwrap_or_else(|error| unreachable!("blocked exec: {error}"));
    assert_eq!(manifest.is_guest_exec_enabled(), Some(false));
}

#[test]
fn an_unknown_guest_contract_is_named_apart_from_the_schema() {
    let mut document = fixture_document();
    for version in [0, 3, 255] {
        document["guest"]["guest_contract_version"] = version.into();
        let Err(error) = BaseImageManifest::parse(&encode(&document)) else {
            unreachable!("contract {version} was accepted");
        };
        assert!(
            error.to_string().contains("guest.guest_contract_version"),
            "contract {version} reported as {error}"
        );
    }
    document["guest"]["guest_contract_version"] = 2.into();
    document["guest"]["schema_version"] = 2.into();
    let Err(error) = BaseImageManifest::parse(&encode(&document)) else {
        unreachable!("an unknown guest schema was accepted");
    };
    assert!(error.to_string().contains("guest.schema_version"));
}

#[test]
fn the_agent_block_list_and_job_account_are_bounded() {
    let mut document = fixture_document();
    for value in [
        "guest-exec; rm -rf /",
        "Guest-Exec",
        "guest exec",
        &"a".repeat(513),
    ] {
        document["guest"]["guest_agent"]["block_rpcs"] = value.into();
        assert!(
            BaseImageManifest::parse(&encode(&document)).is_err(),
            "accepted block list {value}"
        );
    }
    document["guest"]["guest_agent"]["block_rpcs"] = "".into();
    assert!(BaseImageManifest::parse(&encode(&document)).is_ok());

    for (name, uid) in [
        ("runner", 999_u32),
        ("root", 0),
        ("-runner", 2_000),
        (".x", 2_000),
    ] {
        document["guest"]["job_account"] = serde_json::json!({"name": name, "uid": uid});
        assert!(
            BaseImageManifest::parse(&encode(&document)).is_err(),
            "accepted job account {name}:{uid}"
        );
    }
}
