use flanforge_core::{
    Allocation, AllocationMode, CloneKind, CloneSource, FallbackReason, Profile, RunnerLabel,
    VmName,
};
use flanforge_libvirt_wire::{PublishedWarm, VolumePointer};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use flanforge_core::GuestChannelKind;

use crate::image::GuestContract;

use super::{
    ensure_not_cancelled,
    source::{effective_storage_bytes, is_declared_source, warm_pointer_of},
};

const MIB: u64 = 1_048_576;

#[test]
fn cancellation_is_observed_at_every_mutation_boundary() {
    let active = CancellationToken::new();
    for boundary in [
        "before guest resource creation",
        "after guest resource creation",
        "after domain definition",
        "after domain start",
    ] {
        assert!(ensure_not_cancelled(&active, boundary).is_ok());
    }

    active.cancel();
    for boundary in [
        "before guest resource creation",
        "after guest resource creation",
        "after domain definition",
        "after domain start",
    ] {
        assert!(ensure_not_cancelled(&active, boundary).is_err());
    }
}

/// The authorization check on a create: an allocation record is durable state,
/// so only this profile's own two declared names may direct a guest at an
/// image. Anything else boots the cold template.
#[test]
fn only_a_profile_s_own_declared_names_authorize_a_source() {
    let profile = warm_profile();
    let cold_only = Profile {
        warm_template: None,
        ..warm_profile()
    };
    for (name, kind, expected, reason) in [
        (
            "flanforge-base",
            CloneKind::Template,
            true,
            "its own template",
        ),
        ("flanforge-warm", CloneKind::Warm, true, "its own warm name"),
        (
            "other-warm",
            CloneKind::Warm,
            false,
            "another profile's warm image",
        ),
        (
            "other-base",
            CloneKind::Template,
            false,
            "another profile's template",
        ),
    ] {
        assert_eq!(
            is_declared_source(&profile, &source(name, kind)),
            expected,
            "{reason}"
        );
    }
    // A profile that declares no warm name authorizes only its template.
    assert!(is_declared_source(
        &cold_only,
        &source("flanforge-base", CloneKind::Template)
    ));
    assert!(!is_declared_source(
        &cold_only,
        &source("flanforge-warm", CloneKind::Warm)
    ));
}

/// A generation the pointer no longer names is the only degradation left here:
/// a profile smaller than the image is sized up to it rather than degraded.
#[test]
fn only_a_repointed_generation_degrades_rather_than_boots() {
    let document = document(40);
    assert_eq!(
        warm_pointer_of(&document, &source("flanforge-warm", CloneKind::Warm)),
        Ok(document.current().clone())
    );
    assert_eq!(
        warm_pointer_of(&document, &source("flanforge-elsewhere", CloneKind::Warm)),
        Err(FallbackReason::Repointed)
    );
}

/// The overlay is `max(storage_mb, base virtual size)`: a profile that asks for
/// less is run at the base size rather than refused, and one that asks for more
/// gets it.
#[test]
fn the_overlay_is_never_smaller_than_the_base_it_backs_onto() {
    let allocation = allocation();
    let base = 40 * 1_024 * MIB;
    for (storage_mb, expected, reason) in [
        (40 * 1_024, base, "an exactly-sized profile"),
        (
            80 * 1_024,
            80 * 1_024 * MIB,
            "a profile larger than its base",
        ),
        (20 * 1_024, base, "a profile smaller than its base"),
    ] {
        assert_eq!(
            effective_storage_bytes(&allocation, storage_mb, base).ok(),
            Some(expected),
            "{reason}"
        );
    }
    assert!(
        effective_storage_bytes(&allocation, u64::MAX, base).is_err(),
        "a size that overflows bytes is refused rather than truncated"
    );

    // A base that is not a whole MiB still round-trips: admission records the
    // rounded-up size, and the overlay must be created at exactly that, because
    // a warm capture declares the recorded size and a pointer that disagreed
    // with its volume would refuse every later boot.
    let ragged = base + 1;
    let recorded = ragged.div_ceil(MIB);
    assert_eq!(
        effective_storage_bytes(&allocation, recorded, ragged).ok(),
        Some(recorded * MIB)
    );
}

fn allocation() -> Allocation {
    Allocation::new(
        flanforge_test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("flanforged-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        flanforge_test_support::size(),
    )
}

fn warm_profile() -> Profile {
    Profile {
        warm_template: Some(
            VmName::new("flanforge-warm").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        ..flanforge_test_support::profile()
    }
}

fn source(name: &str, kind: CloneKind) -> CloneSource {
    CloneSource {
        name: VmName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        kind,
        base_fingerprint: None,
        fallback_reason: None,
    }
}

fn document(virtual_gib: u64) -> PublishedWarm {
    let pointer = VolumePointer::new(
        "flanforge".to_owned(),
        "warm-00000000-0000-0000-0000-000000000001.qcow2".to_owned(),
        "/pool/warm-00000000-0000-0000-0000-000000000001.qcow2".to_owned(),
        virtual_gib * 1_024 * 1_024 * 1_024,
        1,
        1_755_324_251,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    PublishedWarm::new(
        "project".to_owned(),
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        pointer.produced_at_unix(),
        pointer,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

/// The pre-boot gate: a base that can never satisfy the agent channel is
/// refused before a domain exists, with a cause naming what to rebuild.
#[test]
fn the_agent_channel_refuses_a_base_that_predates_its_contract() {
    let contract = GuestContract::read(&published(|guest| {
        guest["guest_contract_version"] = 1.into();
        let guest = guest
            .as_object_mut()
            .unwrap_or_else(|| unreachable!("fixture: guest is not an object"));
        guest.remove("guest_agent");
        guest.remove("job_account");
    }));
    assert!(!contract.is_readiness_gate_baked());
    assert!(
        contract
            .ensure_channel_supported(GuestChannelKind::Ssh, "runner")
            .is_ok(),
        "SSH must keep working against every base that works today"
    );
    let Err(error) = contract.ensure_channel_supported(GuestChannelKind::Agent, "runner") else {
        unreachable!("a contract-1 base was accepted for the agent channel");
    };
    assert!(
        error
            .to_string()
            .contains("predates the guest agent contract")
    );
}

#[test]
fn the_agent_channel_refuses_a_base_that_blocks_the_exec_rpc() {
    let contract = GuestContract::read(&published(|guest| {
        guest["guest_agent"]["exec_enabled"] = false.into();
    }));
    let Err(error) = contract.ensure_channel_supported(GuestChannelKind::Agent, "runner") else {
        unreachable!("a base blocking guest-exec was accepted");
    };
    assert!(error.to_string().contains("guest-exec blocked"));
}

/// A manifest naming another account would silently run every job somewhere
/// the image never prepared.
#[test]
fn the_agent_channel_refuses_a_job_account_the_operator_did_not_configure() {
    let contract = GuestContract::read(&published(|_| {}));
    assert!(
        contract
            .ensure_channel_supported(GuestChannelKind::Agent, "runner")
            .is_ok()
    );
    let Err(error) = contract.ensure_channel_supported(GuestChannelKind::Agent, "builder") else {
        unreachable!("a mismatched job account was accepted");
    };
    assert!(error.to_string().contains("not the configured builder"));
}

/// Importing a qcow2 built by other means is a documented workflow, so a base
/// with no guest object is probed at run time rather than refused. The agent
/// channel still cannot use one, because it has no account to drop to.
#[test]
fn a_manifest_less_base_keeps_working_and_skips_the_readiness_gate() {
    let manifest = flanforge_libvirt_wire::BaseImageManifest::from_image(
        "custom.qcow2".to_owned(),
        "qcow2".to_owned(),
        "4bd5099b11c4c1aafbbd85c9e5b18b4a891516a7386b4a7997f37983fa181b13".to_owned(),
        14,
        42_949_672_960,
    );
    let contract = GuestContract::read(&publication(manifest));
    assert!(!contract.is_readiness_gate_baked());
    assert!(
        contract
            .ensure_channel_supported(GuestChannelKind::Ssh, "runner")
            .is_ok()
    );
    assert!(
        contract
            .ensure_channel_supported(GuestChannelKind::Agent, "runner")
            .is_err()
    );
}

#[test]
fn a_contract_two_base_bakes_the_readiness_gate_for_both_channels() {
    let contract = GuestContract::read(&published(|_| {}));
    assert!(contract.is_readiness_gate_baked());
}

const TEMPLATE_FIXTURE: &[u8] =
    include_bytes!("../../../../../templates/libvirt/tests/fixtures/image-manifest.json");

fn published(mutate: impl FnOnce(&mut serde_json::Value)) -> flanforge_libvirt_wire::PublishedBase {
    let mut document: serde_json::Value = serde_json::from_slice(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    mutate(&mut document["guest"]);
    let encoded =
        serde_json::to_vec(&document).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    publication(
        flanforge_libvirt_wire::BaseImageManifest::parse(&encoded)
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

fn publication(
    manifest: flanforge_libvirt_wire::BaseImageManifest,
) -> flanforge_libvirt_wire::PublishedBase {
    flanforge_libvirt_wire::PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-aabbcc.qcow2".to_owned(),
        "/pool/flanforge-base-aabbcc.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}
