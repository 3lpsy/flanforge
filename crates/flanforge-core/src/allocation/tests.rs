use std::collections::BTreeSet;

use super::*;
use crate::{NetworkMode, Profile, ProfileName, RepositoryName, RunnerLabel, VmName};
use validator::Validate;

fn allocation() -> Allocation {
    Allocation::new(
        AllocationRequest {
            profile: ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}")),
            repository: RepositoryName::new("owner/halogen")
                .unwrap_or_else(|error| unreachable!("{error}")),
            run_id: 42,
            run_attempt: 1,
        },
        VmName::new("ci-halogen-42-1").unwrap_or_else(|error| unreachable!("{error}")),
        RunnerLabel::new("macos-tart-halogen-allocation")
            .unwrap_or_else(|error| unreachable!("{error}")),
        AllocationMode::Cold,
        GuestSize {
            cpu_count: 4,
            memory_mb: 8_192,
            storage_mb: 40_960,
        },
    )
}

fn profile() -> Profile {
    Profile {
        repository: RepositoryName::new("owner/halogen")
            .unwrap_or_else(|error| unreachable!("{error}")),
        template: VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("{error}")),
        runner_label: RunnerLabel::new("macos-tart-halogen")
            .unwrap_or_else(|error| unreachable!("{error}")),
        job_name: "apple-build".into(),
        allowed_workflows: BTreeSet::from(["apple.yml".to_owned(), "warm.yml".to_owned()]),
        allowed_events: BTreeSet::from(["push".to_owned()]),
        allowed_refs: BTreeSet::from(["refs/heads/main".to_owned()]),
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 8,
        memory_mb: 12_288,
        storage_mb: 40_960,
        boot_timeout_seconds: 300,
        idle_timeout_seconds: 600,
        job_timeout_seconds: 7_200,
        cleanup_timeout_seconds: 120,
        warm_template: None,
        regeneration_workflow: None,
        hot: None,
        reap: true,
    }
}

#[test]
fn lifecycle_accepts_the_success_path() {
    let mut allocation = allocation();
    for state in [
        AllocationState::Preparing,
        AllocationState::Booting,
        AllocationState::Registering,
        AllocationState::WaitingForJob,
        AllocationState::Ready,
        AllocationState::Running,
        AllocationState::Cleaning,
        AllocationState::Completed,
    ] {
        assert_eq!(allocation.transition(state), Ok(()));
    }
    assert!(allocation.state.is_terminal());
}

#[test]
fn lifecycle_requires_cleanup_before_a_terminal_state() {
    let mut allocation = allocation();
    let result = allocation.transition(AllocationState::Failed);
    assert_eq!(
        result,
        Err(StateTransitionError {
            from: AllocationState::Requested,
            to: AllocationState::Failed,
        })
    );
}

#[test]
fn terminal_state_cannot_restart() {
    let mut allocation = allocation();
    assert_eq!(allocation.transition(AllocationState::Cleaning), Ok(()));
    assert_eq!(allocation.transition(AllocationState::Cancelled), Ok(()));
    assert!(allocation.transition(AllocationState::Preparing).is_err());
}

#[test]
fn persisted_error_is_bounded() {
    let mut allocation = allocation();
    allocation.set_error("x".repeat(1_024));
    assert_eq!(allocation.error.as_deref().map(str::len), Some(512));
}

#[test]
fn allocation_request_requires_positive_run_identity() {
    let mut allocation = allocation();
    allocation.request.run_id = 0;
    assert!(allocation.validate().is_err());

    allocation.request.run_id = 42;
    allocation.request.run_attempt = 0;
    assert!(allocation.validate().is_err());
}

#[test]
fn resolve_size_defaults_to_the_profile_and_rejects_over_ceiling() {
    let profile = profile();
    assert_eq!(
        resolve_size(RequestOptions::default(), &profile),
        Ok(GuestSize {
            cpu_count: 8,
            memory_mb: 12_288,
            storage_mb: 40_960,
        })
    );
    assert_eq!(
        resolve_size(
            RequestOptions {
                warm: false,
                hot: crate::HotRequest::Untouched,
                cpu_count: Some(2),
                memory_mb: None,
            },
            &profile,
        ),
        Ok(GuestSize {
            cpu_count: 2,
            memory_mb: 12_288,
            storage_mb: 40_960,
        })
    );
    for over in [
        RequestOptions {
            warm: false,
            hot: crate::HotRequest::Untouched,
            cpu_count: Some(9),
            memory_mb: None,
        },
        RequestOptions {
            warm: false,
            hot: crate::HotRequest::Untouched,
            cpu_count: None,
            memory_mb: Some(12_289),
        },
    ] {
        assert_eq!(resolve_size(over, &profile), Err(SizeCeilingError));
    }
}

#[test]
fn resolve_mode_ignores_a_warm_request_for_the_regeneration_workflow() {
    let mut profile = profile();
    let warm = RequestOptions {
        warm: true,
        hot: crate::HotRequest::Untouched,
        cpu_count: None,
        memory_mb: None,
    };
    assert_eq!(
        resolve_mode(RequestOptions::default(), &profile, "apple.yml"),
        AllocationMode::Cold
    );
    assert_eq!(
        resolve_mode(warm, &profile, "apple.yml"),
        AllocationMode::Warm
    );

    profile.warm_template =
        Some(VmName::new("halogen-warm").unwrap_or_else(|error| unreachable!("{error}")));
    profile.regeneration_workflow = Some("warm.yml".to_owned());
    assert_eq!(
        resolve_mode(warm, &profile, "warm.yml"),
        AllocationMode::Regenerate
    );
    assert_eq!(
        resolve_mode(RequestOptions::default(), &profile, "warm.yml"),
        AllocationMode::Regenerate
    );
    assert_eq!(
        resolve_mode(warm, &profile, "apple.yml"),
        AllocationMode::Warm
    );
}

#[test]
fn only_a_regeneration_that_created_a_vm_can_retain() {
    let mut allocation = allocation();
    assert!(!allocation.is_retention_eligible());
    allocation.mode = AllocationMode::Regenerate;
    assert!(!allocation.is_retention_eligible());
    allocation.set_vm_created();
    assert!(allocation.is_retention_eligible());
}

#[test]
fn warm_generation_is_optional_and_cleared_by_a_cold_fallback() {
    let mut allocation = allocation();
    allocation.set_clone_source(
        CloneSource {
            name: VmName::new("project-warm")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            kind: CloneKind::Warm,
            base_fingerprint: None,
            fallback_reason: None,
        },
        Some(7),
    );
    assert_eq!(allocation.warm_generation, Some(7));

    allocation.set_clone_source(
        CloneSource {
            name: VmName::new("project-base")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            kind: CloneKind::Template,
            base_fingerprint: None,
            fallback_reason: Some(FallbackReason::Absent),
        },
        Some(8),
    );
    assert_eq!(allocation.warm_generation, None);
}

/// The tolerant reader exists so one unknown reason cannot fail a whole
/// allocation listing. An exhaustive match makes adding a variant a compile
/// error here rather than a value every reader silently prints as "unknown".
#[test]
fn every_fallback_reason_round_trips_and_an_unknown_one_decodes_as_unknown() {
    for reason in [
        FallbackReason::NotDeclared,
        FallbackReason::NoRecord,
        FallbackReason::Repointed,
        FallbackReason::Absent,
        FallbackReason::NotStopped,
        FallbackReason::StaleBase,
        FallbackReason::Unclaimed,
        FallbackReason::Quarantined,
        FallbackReason::NotPromoted,
        FallbackReason::FingerprintUnavailable,
        FallbackReason::Unsupported,
        FallbackReason::StorageTooSmall,
        FallbackReason::Unknown,
    ] {
        // The exhaustive match is the point: a new variant fails to compile
        // until it is listed above and in the deserializer's own table.
        let encoded = match reason {
            FallbackReason::NotDeclared
            | FallbackReason::NoRecord
            | FallbackReason::Repointed
            | FallbackReason::Absent
            | FallbackReason::NotStopped
            | FallbackReason::StaleBase
            | FallbackReason::Unclaimed
            | FallbackReason::Quarantined
            | FallbackReason::NotPromoted
            | FallbackReason::FingerprintUnavailable
            | FallbackReason::Unsupported
            | FallbackReason::StorageTooSmall
            | FallbackReason::Unknown => serde_json::to_string(&reason)
                .unwrap_or_else(|error| unreachable!("encode: {error}")),
        };
        let decoded = serde_json::from_str::<FallbackReason>(&encoded)
            .unwrap_or_else(|error| unreachable!("decode {encoded}: {error}"));
        // `Unknown` is the only variant the writer must never emit as itself
        // and the reader still has to accept.
        assert_eq!(decoded, reason, "{encoded}");
    }
    assert_eq!(
        serde_json::from_str::<FallbackReason>("\"a_reason_from_a_later_build\"")
            .unwrap_or_else(|error| unreachable!("decode: {error}")),
        FallbackReason::Unknown
    );
    assert!(serde_json::from_str::<FallbackReason>("7").is_err());
}

/// Every state, in one place so the matrix below can sweep all of them. A new
/// variant has to be added here and given a row in `legal_destinations`, which
/// will not compile until it does.
const EVERY_STATE: [AllocationState; 11] = [
    AllocationState::Requested,
    AllocationState::Preparing,
    AllocationState::Booting,
    AllocationState::Registering,
    AllocationState::WaitingForJob,
    AllocationState::Ready,
    AllocationState::Running,
    AllocationState::Cleaning,
    AllocationState::Completed,
    AllocationState::Failed,
    AllocationState::Cancelled,
];

/// Where each state may move to, restated independently of the rule so that
/// editing the rule has to change this table as well. Self-transitions are
/// idempotent by rule and are asserted separately, so no row lists itself.
fn legal_destinations(state: AllocationState) -> &'static [AllocationState] {
    match state {
        AllocationState::Requested => &[AllocationState::Preparing, AllocationState::Cleaning],
        // Registering is the hot claim: the machine is already up, so the
        // clone, the boot, and the readiness wait are all skipped.
        AllocationState::Preparing => &[
            AllocationState::Booting,
            AllocationState::Registering,
            AllocationState::Cleaning,
        ],
        AllocationState::Booting => &[AllocationState::Registering, AllocationState::Cleaning],
        AllocationState::Registering => {
            &[AllocationState::WaitingForJob, AllocationState::Cleaning]
        }
        AllocationState::WaitingForJob => &[
            AllocationState::Ready,
            AllocationState::Running,
            AllocationState::Cleaning,
        ],
        AllocationState::Ready => &[AllocationState::Running, AllocationState::Cleaning],
        AllocationState::Running => &[AllocationState::Cleaning],
        AllocationState::Cleaning => &[
            AllocationState::Completed,
            AllocationState::Failed,
            AllocationState::Cancelled,
        ],
        AllocationState::Completed | AllocationState::Failed | AllocationState::Cancelled => &[],
    }
}

/// The whole 11x11 matrix. Targeted lifecycle tests cover the paths that are
/// walked in production; this covers the pairs nothing walks, so a rule edit
/// cannot quietly open a skip, a rewind, or a terminal-state restart.
#[test]
fn every_state_pair_transitions_exactly_as_tabulated() {
    for from in EVERY_STATE {
        for to in EVERY_STATE {
            let is_legal = from == to || legal_destinations(from).contains(&to);
            assert_eq!(
                from.can_transition_to(to),
                is_legal,
                "{from:?} -> {to:?} disagrees with the table"
            );
        }
    }
}

/// Workers re-report the state they are already in, so a repeat must succeed
/// rather than fail the allocation.
#[test]
fn every_state_can_transition_to_itself() {
    for state in EVERY_STATE {
        assert!(state.can_transition_to(state), "{state:?}");
    }
}

/// Ties `is_terminal` to the matrix: a terminal state is exactly one with no
/// way out, so adding an edge to a terminal state has to move it out of the
/// terminal set too.
#[test]
fn a_terminal_state_is_exactly_a_state_with_no_destination() {
    for state in EVERY_STATE {
        assert_eq!(
            state.is_terminal(),
            legal_destinations(state).is_empty(),
            "{state:?}"
        );
        // A terminal state also refuses every other state, including the other
        // terminal ones, so nothing can be reclassified after the fact.
        if state.is_terminal() {
            for other in EVERY_STATE.into_iter().filter(|other| *other != state) {
                assert!(!state.can_transition_to(other), "{state:?} -> {other:?}");
            }
        }
    }
}

/// Cancellation drives any live allocation to `Cleaning`, and cleanup is the
/// only route into a terminal state. A new state that broke either would leave
/// cancellation silently unable to finish.
#[test]
fn cleanup_is_the_only_route_out_of_every_live_state() {
    for state in EVERY_STATE.into_iter().filter(|state| !state.is_terminal()) {
        assert!(
            state.can_transition_to(AllocationState::Cleaning),
            "{state:?} cannot be cancelled"
        );
        for terminal in EVERY_STATE.into_iter().filter(|state| state.is_terminal()) {
            assert_eq!(
                state.can_transition_to(terminal),
                state == AllocationState::Cleaning,
                "{state:?} -> {terminal:?}"
            );
        }
    }
}

/// The matrix again through `Allocation::transition`, which is what production
/// calls, and which must leave the record untouched when it refuses.
#[test]
fn a_refused_transition_reports_the_pair_and_changes_nothing() {
    for from in EVERY_STATE {
        for to in EVERY_STATE {
            let mut allocation = allocation();
            allocation.state = from;
            let before = allocation.clone();
            let result = allocation.transition(to);
            if from == to || legal_destinations(from).contains(&to) {
                assert_eq!(result, Ok(()), "{from:?} -> {to:?}");
                assert_eq!(allocation.state, to);
            } else {
                assert_eq!(result, Err(StateTransitionError { from, to }));
                assert_eq!(allocation, before, "{from:?} -> {to:?} mutated the record");
            }
        }
    }
}

/// The ceiling a base-image floor clamps to has to be a size a record can
/// actually carry, so it is the validated maximum and not a number beside it.
#[test]
fn the_storage_ceiling_is_the_largest_size_that_validates() {
    let size = GuestSize {
        cpu_count: 4,
        memory_mb: 8_192,
        storage_mb: GuestSize::MAX_STORAGE_MB,
    };
    assert!(size.validate().is_ok());
    assert!(
        GuestSize {
            storage_mb: GuestSize::MAX_STORAGE_MB + 1,
            ..size
        }
        .validate()
        .is_err()
    );
}
