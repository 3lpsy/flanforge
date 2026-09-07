//! Record fixtures shared by this crate's test modules.

use flanforge_core::{
    Allocation, AllocationMode, BaseFingerprint, CloneKind, CloneSource, HotGuest, HotLane, VmName,
    WarmGeneration, WarmImageRecord, WarmImageState,
};

pub(crate) fn vm_name(value: &str) -> VmName {
    VmName::new(value).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

pub(crate) fn fingerprint() -> BaseFingerprint {
    BaseFingerprint::new("0000aaaa").unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

pub(crate) fn clone_source() -> CloneSource {
    CloneSource {
        name: vm_name("project-warm"),
        kind: CloneKind::Warm,
        base_fingerprint: Some(fingerprint()),
        fallback_reason: None,
    }
}

pub(crate) fn allocation() -> Allocation {
    Allocation::new(
        flanforge_test_support::request(),
        vm_name("ci-guest"),
        flanforge_test_support::profile().runner_label,
        AllocationMode::Warm,
        flanforge_test_support::size(),
    )
}

pub(crate) fn hot_guest() -> HotGuest {
    HotGuest::new(
        vm_name("ci-hot"),
        flanforge_test_support::profile_name(),
        HotLane::Protected,
        flanforge_test_support::size(),
        clone_source(),
        Some(7),
        Some(3_600),
    )
}

pub(crate) fn warm_image() -> WarmImageRecord {
    WarmImageRecord {
        profile: flanforge_test_support::profile_name(),
        warm_template: vm_name("project-warm"),
        generation: 7,
        base_fingerprint: fingerprint(),
        produced_by: allocation().id,
        produced_at_unix: 1_000,
        state: WarmImageState::Promoted,
        previous: Some(WarmGeneration {
            generation: 6,
            base_fingerprint: fingerprint(),
            produced_by: allocation().id,
            produced_at_unix: 900,
        }),
    }
}
