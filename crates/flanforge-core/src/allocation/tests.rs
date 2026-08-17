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
        allowed_ref_prefixes: BTreeSet::new(),
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 8,
        memory_mb: 12_288,
        boot_timeout_seconds: 300,
        idle_timeout_seconds: 600,
        job_timeout_seconds: 7_200,
        cleanup_timeout_seconds: 120,
        warm_template: None,
        regeneration_workflow: None,
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
        })
    );
    assert_eq!(
        resolve_size(
            RequestOptions {
                warm: false,
                cpu_count: Some(2),
                memory_mb: None,
            },
            &profile,
        ),
        Ok(GuestSize {
            cpu_count: 2,
            memory_mb: 12_288,
        })
    );
    for over in [
        RequestOptions {
            warm: false,
            cpu_count: Some(9),
            memory_mb: None,
        },
        RequestOptions {
            warm: false,
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
