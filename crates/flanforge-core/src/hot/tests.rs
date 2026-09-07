use validator::Validate;

use super::{HotGuest, HotLane, HotLanePolicy, HotState};
use crate::{AllocationId, CloneKind, CloneSource, GuestSize, ProfileName, VmName};

fn guest() -> HotGuest {
    HotGuest::new(
        VmName::new("ci-hot-project-0123456789ab")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ProfileName::new("project").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        HotLane::Protected,
        GuestSize {
            cpu_count: 4,
            memory_mb: 8_192,
            storage_mb: 40_960,
        },
        CloneSource {
            name: VmName::new("flanforge-base")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            kind: CloneKind::Template,
            base_fingerprint: None,
            fallback_reason: None,
        },
        None,
        None,
    )
}

#[test]
fn the_reuse_loop_and_the_polite_exit_are_the_only_ways_out_of_a_claim() {
    let mut hot = guest();
    hot.ensure_state(HotState::Idle)
        .unwrap_or_else(|error| unreachable!("provisioned: {error}"));
    hot.ensure_claimed(AllocationId::new())
        .unwrap_or_else(|error| unreachable!("claim: {error}"));
    hot.ensure_released()
        .unwrap_or_else(|error| unreachable!("release: {error}"));
    assert_eq!(hot.state, HotState::Recycling);
    assert_eq!(hot.jobs_served, 1);
    assert!(hot.claimed_by.is_none());
    hot.ensure_state(HotState::Idle)
        .unwrap_or_else(|error| unreachable!("recycled: {error}"));

    assert!(HotState::Claimed.can_transition_to(HotState::Draining));
    assert!(HotState::Draining.can_transition_to(HotState::Evicted));
    // A claimed machine leaves through Draining, so the record always shows
    // the claim ending before the machine does.
    assert!(!HotState::Claimed.can_transition_to(HotState::Evicted));
    for state in [
        HotState::Provisioning,
        HotState::Idle,
        HotState::Claimed,
        HotState::Recycling,
        HotState::Draining,
    ] {
        assert!(!HotState::Evicted.can_transition_to(state), "{state:?}");
    }
}

#[test]
fn a_claim_outside_the_claimed_state_never_survives_validation() {
    let mut hot = guest();
    hot.ensure_state(HotState::Idle)
        .unwrap_or_else(|error| unreachable!("provisioned: {error}"));
    assert!(hot.validate().is_ok());

    hot.claimed_by = Some(AllocationId::new());
    assert!(
        hot.validate().is_err(),
        "an Idle record cannot hold a claim"
    );

    hot.state = HotState::Draining;
    assert!(hot.validate().is_err(), "a drain cannot hold a claim");

    hot.state = HotState::Claimed;
    assert!(hot.validate().is_ok());

    hot.claimed_by = None;
    assert!(hot.validate().is_err(), "a Claimed record needs its claim");
}

#[test]
fn lane_policy_admits_only_the_lanes_it_names() {
    for (policy, protected, unprotected) in [
        (HotLanePolicy::None, false, false),
        (HotLanePolicy::Protected, true, false),
        (HotLanePolicy::Any, true, true),
    ] {
        assert_eq!(policy.is_lane_admitted(HotLane::Protected), protected);
        assert_eq!(policy.is_lane_admitted(HotLane::Unprotected), unprotected);
    }
    assert_eq!(HotLane::from_ref_protected(true), HotLane::Protected);
    assert_eq!(HotLane::from_ref_protected(false), HotLane::Unprotected);
}

#[test]
fn a_machine_serves_only_its_own_profile_and_lane_while_idle() {
    let other = ProfileName::new("other").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut hot = guest();
    let profile = hot.profile.clone();
    assert!(
        !hot.is_serving(&profile, HotLane::Protected),
        "a provisioning machine is not claimable"
    );
    hot.ensure_state(HotState::Idle)
        .unwrap_or_else(|error| unreachable!("provisioned: {error}"));
    assert!(hot.is_serving(&profile, HotLane::Protected));
    assert!(!hot.is_serving(&profile, HotLane::Unprotected));
    assert!(!hot.is_serving(&other, HotLane::Protected));
}
