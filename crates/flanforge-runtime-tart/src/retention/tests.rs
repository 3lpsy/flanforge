use std::{sync::Arc, time::Duration};

use flanforge_core::{
    Allocation, AllocationMode, BaseFingerprint, Profile, RetentionPhase, RetentionResult,
    RunnerLabel, VmName, WarmGeneration,
};

use flanforge_forgejo::ForgejoClient;
use flanforge_manager::WorkerError;
use flanforge_test_support as test_support;

use super::{
    super::worker::FlanForgeWorker,
    request::{Names, RetentionRequest, Stopped, derive},
    steps::{FailedAuthority, failed_authority},
};

/// The host as the rollback finds it, plus the mutations it performs.
struct Fixture {
    worker: FlanForgeWorker,
    profile: Profile,
    allocation: Allocation,
    names: Names,
    log: std::path::PathBuf,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new(present: &[&str]) -> Self {
        Self::refusing(present, "")
    }

    /// `failing` names the Tart subcommand the fake refuses.
    fn refusing(present: &[&str], failing: &str) -> Self {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let listing = present
            .iter()
            .map(|name| format!(r#"{{"Name":"{name}","State":"stopped"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let (script, log) =
            crate::tests::fake_tart_failing(&directory, &format!("[{listing}]"), failing);
        let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
        config
            .runtime
            .tart_mut()
            .unwrap_or_else(|| unreachable!("Tart fixture"))
            .path = script;
        let worker = FlanForgeWorker::new(
            &config,
            ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        let warm =
            VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}"));
        Self {
            worker,
            profile: test_support::warm_profile(),
            allocation: Allocation::new(
                test_support::request(),
                VmName::new("ci-project-42-1")
                    .unwrap_or_else(|error| unreachable!("fixture: {error}")),
                RunnerLabel::new("macos-tart-project-retention")
                    .unwrap_or_else(|error| unreachable!("fixture: {error}")),
                AllocationMode::Regenerate,
                test_support::size(),
            ),
            names: derive(&warm).unwrap_or_else(|error| unreachable!("fixture: {error}")),
            log,
            _directory: directory,
        }
    }

    fn request(&self) -> RetentionRequest<'_> {
        RetentionRequest {
            allocation: &self.allocation,
            profile: &self.profile,
            warm_template: self.names.warm.clone(),
            base_fingerprint: fingerprint("aa01"),
            generation: 5,
            previous: Some(WarmGeneration {
                generation: 4,
                base_fingerprint: fingerprint("9900"),
                produced_by: flanforge_core::AllocationId::new(),
                produced_at_unix: 1_755_000_000,
            }),
        }
    }

    fn mutations(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn fingerprint(value: &str) -> BaseFingerprint {
    BaseFingerprint::new(value).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn stopped(
    phase: RetentionPhase,
    error: &WorkerError,
    deadline: tokio::time::Instant,
) -> Stopped<'_> {
    Stopped {
        phase,
        error,
        deadline,
    }
}

/// CORE-525: the staging-record write destroys nothing, so its phase must not
/// send the rollback down the branch that assumes the live image is gone.
#[test]
fn failure_phase_and_live_name_identify_the_surviving_generation() {
    for untouched in [
        RetentionPhase::Strip,
        RetentionPhase::Stop,
        RetentionPhase::Stage,
        RetentionPhase::Verify,
    ] {
        assert_eq!(
            failed_authority(untouched, true),
            FailedAuthority::Unchanged,
            "{untouched:?}"
        );
    }
    assert_eq!(
        failed_authority(RetentionPhase::Retire, true),
        FailedAuthority::PreviousLive
    );
    assert_eq!(
        failed_authority(RetentionPhase::Promote, true),
        FailedAuthority::CandidateLive
    );
    for phase in [RetentionPhase::Retire, RetentionPhase::Promote] {
        assert_eq!(failed_authority(phase, false), FailedAuthority::LiveAbsent);
    }
}

/// CORE-525: a failure before retirement touches only the candidate.
#[tokio::test]
async fn a_failure_before_retirement_only_drops_the_candidate() {
    let fixture = Fixture::new(&["project-warm", "project-warm.staging"]);
    let deadline = tokio::time::Instant::now() + Duration::from_mins(1);

    fixture
        .worker
        .drop_candidate(&fixture.request(), &fixture.names, deadline)
        .await;

    assert_eq!(fixture.mutations(), "delete project-warm.staging\n");
}

/// RUN-523: a restore that could not run is never reported as one that did.
#[tokio::test]
async fn a_restore_that_cannot_run_is_reported_as_failed() {
    let error = WorkerError::new("retention retirement exceeded its budget");

    // Nothing survives to restore from.
    let absent = Fixture::new(&[]);
    let reported = absent
        .worker
        .restore_outcome(
            &absent.request(),
            &absent.names,
            stopped(
                RetentionPhase::Retire,
                &error,
                tokio::time::Instant::now() + Duration::from_mins(1),
            ),
        )
        .await;
    assert_eq!(reported.outcome, RetentionResult::Failed);
    assert_eq!(absent.mutations(), "");

    // The previous generation is there but Tart refuses the clone.
    let refusing = Fixture::refusing(&["project-warm.previous"], "clone");
    let reported = refusing
        .worker
        .restore_outcome(
            &refusing.request(),
            &refusing.names,
            stopped(
                RetentionPhase::Retire,
                &error,
                tokio::time::Instant::now() + Duration::from_mins(1),
            ),
        )
        .await;
    assert_eq!(reported.outcome, RetentionResult::Failed);
    assert!(reported.reason.contains("restore failed"), "{reported:?}");
}

/// RUN-522: the rollback acts on the budget it is given, so the exhausted
/// sequence budget must not be the one it inherits.
#[tokio::test]
async fn a_rollback_restores_on_a_live_budget_and_reports_failure_on_a_spent_one() {
    let fixture = Fixture::new(&["project-warm.previous"]);
    let error = WorkerError::new("retention retirement exceeded its budget");

    let spent = fixture
        .worker
        .restore_outcome(
            &fixture.request(),
            &fixture.names,
            stopped(RetentionPhase::Retire, &error, tokio::time::Instant::now()),
        )
        .await;
    assert_eq!(spent.outcome, RetentionResult::Failed);
    assert_eq!(fixture.mutations(), "");

    let live = fixture
        .worker
        .restore_outcome(
            &fixture.request(),
            &fixture.names,
            stopped(
                RetentionPhase::Retire,
                &error,
                tokio::time::Instant::now() + Duration::from_mins(1),
            ),
        )
        .await;
    assert_eq!(live.outcome, RetentionResult::RolledBack);
    assert_eq!(
        fixture.mutations(),
        "clone project-warm.previous project-warm\n"
    );
}

/// A live image the failure never destroyed is left exactly as it is.
#[tokio::test]
async fn a_surviving_live_image_is_never_cloned_over() {
    let fixture = Fixture::new(&["project-warm", "project-warm.previous"]);
    let error = WorkerError::new("retention promotion exceeded its budget");

    let reported = fixture
        .worker
        .restore_outcome(
            &fixture.request(),
            &fixture.names,
            stopped(
                RetentionPhase::Promote,
                &error,
                tokio::time::Instant::now() + Duration::from_mins(1),
            ),
        )
        .await;
    assert_eq!(reported.outcome, RetentionResult::RolledBack);
    assert_eq!(fixture.mutations(), "");
}
