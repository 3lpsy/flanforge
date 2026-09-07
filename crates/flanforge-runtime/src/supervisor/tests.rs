use flanforge_core::{Allocation, AllocationMode, RunnerLabel, VmName};
use flanforge_forgejo::RunnerStatus;

use super::SupervisionWindow;
use crate::channel::GuestExit;

fn allocation() -> Allocation {
    Allocation::new(
        flanforge_test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        flanforge_test_support::size(),
    )
}

fn window() -> SupervisionWindow {
    SupervisionWindow::new(
        &flanforge_test_support::profile(),
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}

/// A runner that got as far as a job owns no verdict on it, so only one that
/// never reached a job can fail the allocation.
#[tokio::test]
async fn only_an_exit_before_a_job_fails_the_allocation() {
    let allocation = allocation();
    let waiting = window();
    waiting
        .ensure_exit_is_a_verdict(GuestExit::Code(0), &allocation)
        .unwrap_or_else(|error| unreachable!("successful exit: {error}"));
    for exit in [GuestExit::Code(1), GuestExit::Signal(15)] {
        assert!(
            waiting.ensure_exit_is_a_verdict(exit, &allocation).is_err(),
            "{exit:?}"
        );
    }

    let mut running = window();
    running.advance(RunnerStatus::Active);
    assert!(running.is_running());
    running
        .ensure_exit_is_a_verdict(GuestExit::Code(1), &allocation)
        .unwrap_or_else(|error| unreachable!("exit after a job: {error}"));
}

/// A lost observation is not an exit: the process may still be running, and
/// reporting it as one would run retention over a live job.
#[tokio::test]
async fn a_lost_observation_is_never_a_verdict() {
    let allocation = allocation();
    let mut running = window();
    running.advance(RunnerStatus::Active);
    for window in [window(), running] {
        let error = window
            .ensure_exit_is_a_verdict(GuestExit::Lost, &allocation)
            .err()
            .unwrap_or_else(|| unreachable!("a lost observation must fail supervision"));
        assert_eq!(error.to_string(), "guest runner became unobservable");
    }
}
