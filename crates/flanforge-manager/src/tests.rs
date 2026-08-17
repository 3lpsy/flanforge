use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use flanforge_core::{Allocation, AllocationId, AllocationState, Profile, RequestOptions};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    AllocationManager, AllocationReporter, AllocationWorker, ConfigHandle, ManagerError,
    WorkerError,
    service::{Entry, IN_MEMORY_ALLOCATION_LIMIT},
    state::prune_terminal_history,
};

#[derive(Debug)]
struct WaitingWorker {
    cleanups: Arc<AtomicUsize>,
    cleanup_failures: usize,
}

impl WaitingWorker {
    fn new(cleanups: Arc<AtomicUsize>, cleanup_failures: usize) -> Self {
        Self {
            cleanups,
            cleanup_failures,
        }
    }
}

#[async_trait]
impl AllocationWorker for WaitingWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        for state in [
            AllocationState::Preparing,
            AllocationState::Booting,
            AllocationState::Registering,
            AllocationState::WaitingForJob,
            AllocationState::Ready,
        ] {
            reporter
                .transition(state)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
        }
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        let attempt = self.cleanups.fetch_add(1, Ordering::SeqCst);
        if attempt < self.cleanup_failures {
            return Err(WorkerError::new("cleanup failed"));
        }
        Ok(())
    }
}

async fn images(directory: &tempfile::TempDir) -> Arc<dyn WarmImageStore> {
    Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

async fn manager() -> (AllocationManager, Arc<AtomicUsize>, tempfile::TempDir) {
    manager_with_cleanup_failures(0).await
}

async fn manager_with_cleanup_failures(
    cleanup_failures: usize,
) -> (AllocationManager, Arc<AtomicUsize>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let cleanups = Arc::new(AtomicUsize::new(0));
    let worker = Arc::new(WaitingWorker::new(cleanups.clone(), cleanup_failures));
    (
        AllocationManager::new(
            ConfigHandle::new(config),
            store,
            images(&directory).await,
            worker,
        ),
        cleanups,
        directory,
    )
}

async fn await_terminal(manager: &AllocationManager, id: AllocationId) -> Allocation {
    for _ in 0..500 {
        let allocation = manager
            .get(id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"));
        if allocation.state.is_terminal() {
            return allocation;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    unreachable!("allocation did not reach a terminal state");
}

#[tokio::test]
async fn creation_is_idempotent_and_cancellation_cleans() {
    let (manager, cleanups, _directory) = manager().await;
    let first = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    let second = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("idempotent create: {error}"));
    assert!(first.is_new);
    assert!(!second.is_new);
    assert_eq!(first.allocation.id, second.allocation.id);

    manager
        .cancel_authorized(first.allocation.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    // The shared budget, so a loaded test run fails on a defect rather than a
    // tight bound.
    let terminal = await_terminal(&manager, first.allocation.id).await;
    assert_eq!(terminal.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_an_unauthorized_repository() {
    let (manager, _cleanups, _directory) = manager().await;
    let mut claims = test_support::claims();
    claims.repository = "owner/other".into();
    assert!(
        manager
            .create(test_support::request(), RequestOptions::default(), &claims)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn shutdown_cancels_and_awaits_active_cleanup() {
    let (manager, cleanups, _directory) = manager().await;
    let allocation = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;
    // The budget only has to exceed cleanup, not measure it: a tight bound
    // fails under a loaded test run rather than on a real defect.
    assert!(manager.shutdown(std::time::Duration::from_secs(30)).await);
    let current = manager
        .get(allocation.id)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert_eq!(current.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert!(matches!(
        manager
            .create(
                test_support::request(),
                RequestOptions::default(),
                &test_support::claims()
            )
            .await,
        Err(super::ManagerError::ShuttingDown)
    ));
}

#[tokio::test]
async fn recovery_keeps_recent_active_work_when_history_exceeds_the_limit() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    for index in 0..IN_MEMORY_ALLOCATION_LIMIT {
        let mut allocation = Allocation::new(
            test_support::request(),
            flanforge_core::VmName::new(format!("ci-old-{index}"))
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            flanforge_core::RunnerLabel::new(format!("old-{index}"))
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            flanforge_core::AllocationMode::Cold,
            test_support::size(),
        );
        allocation.state = AllocationState::Completed;
        let bytes = serde_json::to_vec(&allocation)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        tokio::fs::write(
            directory.path().join(format!("{}.json", allocation.id)),
            bytes,
        )
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let store = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let mut active = Allocation::new(
        test_support::request(),
        flanforge_core::VmName::new("ci-recovery-active")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::RunnerLabel::new("recovery-active")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::AllocationMode::Cold,
        test_support::size(),
    );
    active.state = AllocationState::Preparing;
    store
        .save(&active)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(&directory).await,
        Arc::new(WaitingWorker::new(cleanups.clone(), 0)),
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));

    let recovered = manager
        .get(active.id)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert_eq!(recovered.state, AllocationState::Failed);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[test]
fn terminal_history_is_bounded_without_evicting_active_work() {
    let mut entries = HashMap::new();
    for index in 0..IN_MEMORY_ALLOCATION_LIMIT {
        let mut allocation = Allocation::new(
            test_support::request(),
            flanforge_core::VmName::new(format!("ci-history-{index}"))
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            flanforge_core::RunnerLabel::new(format!("history-{index}"))
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            flanforge_core::AllocationMode::Cold,
            test_support::size(),
        );
        allocation.state = AllocationState::Completed;
        allocation.updated_at_unix = index as u64;
        let (sender, _) = tokio::sync::watch::channel(allocation.clone());
        entries.insert(
            allocation.id,
            Entry {
                allocation,
                sender,
                cancellation: CancellationToken::new(),
                is_supervised: true,
                is_terminalizing: false,
            },
        );
    }
    let active = Allocation::new(
        test_support::request(),
        flanforge_core::VmName::new("ci-active")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::RunnerLabel::new("active")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::AllocationMode::Cold,
        test_support::size(),
    );
    let active_id = active.id;
    let (sender, _) = tokio::sync::watch::channel(active.clone());
    entries.insert(
        active.id,
        Entry {
            allocation: active,
            sender,
            cancellation: CancellationToken::new(),
            is_supervised: true,
            is_terminalizing: false,
        },
    );

    prune_terminal_history(&mut entries);

    assert_eq!(entries.len(), IN_MEMORY_ALLOCATION_LIMIT);
    assert!(entries.contains_key(&active_id));
    assert_eq!(
        entries
            .values()
            .filter(|entry| !entry.allocation.state.is_terminal())
            .count(),
        1
    );
}

#[tokio::test]
async fn exhausted_cleanup_still_terminalizes_and_frees_capacity() {
    let (manager, cleanups, _directory) = manager_with_cleanup_failures(usize::MAX).await;
    let first = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;
    manager
        .cancel_authorized(first.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));

    let terminal = await_terminal(&manager, first.id).await;
    assert_eq!(terminal.state, AllocationState::Failed);
    assert_eq!(cleanups.load(Ordering::SeqCst), 3);
    assert!(
        terminal
            .error
            .is_some_and(|error| error.contains("cleanup failed"))
    );

    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    assert!(
        manager
            .create(request, RequestOptions::default(), &claims)
            .await
            .is_ok()
    );
    assert!(manager.shutdown(std::time::Duration::from_secs(30)).await);
}

#[tokio::test]
async fn cleanup_retries_terminalize_without_a_restart() {
    let (manager, cleanups, _directory) = manager_with_cleanup_failures(2).await;
    let allocation = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;
    manager
        .cancel_authorized(allocation.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));

    let terminal = await_terminal(&manager, allocation.id).await;
    assert_eq!(terminal.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn recovery_reconciles_every_record_when_one_profile_is_missing() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let mut orphan = Allocation::new(
        flanforge_core::AllocationRequest {
            profile: flanforge_core::ProfileName::new("removed")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            ..test_support::request()
        },
        flanforge_core::VmName::new("ci-orphan-record")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::RunnerLabel::new("orphan-record")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::AllocationMode::Cold,
        test_support::size(),
    );
    orphan.state = AllocationState::Booting;
    store
        .save(&orphan)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut active = Allocation::new(
        test_support::request(),
        flanforge_core::VmName::new("ci-valid-record")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::RunnerLabel::new("valid-record")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::AllocationMode::Cold,
        test_support::size(),
    );
    active.state = AllocationState::Preparing;
    store
        .save(&active)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(&directory).await,
        Arc::new(WaitingWorker::new(cleanups.clone(), 0)),
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));

    for id in [active.id, orphan.id] {
        let recovered = manager
            .get(id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"));
        assert_eq!(recovered.state, AllocationState::Failed);
    }
    assert_eq!(cleanups.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_invalid_edit_leaves_the_running_configuration_in_place() {
    let (manager, _cleanups, _directory) = manager().await;
    let handle = Arc::clone(&manager.inner.config);
    let generation = handle.generation();
    let mut invalid = (*handle.current()).clone();
    invalid.profiles.clear();

    assert!(handle.apply(Arc::new(invalid)).is_err());
    assert_eq!(handle.generation(), generation);
    assert_eq!(handle.current().profiles.len(), 1);
    assert!(handle.restart_pending().is_empty());
}

#[tokio::test]
async fn a_restart_only_field_warns_on_every_reload_and_does_not_apply() {
    let (manager, _cleanups, _directory) = manager().await;
    let handle = Arc::clone(&manager.inner.config);
    let mut next = (*handle.current()).clone();
    next.runtime.poll_seconds += 1;
    let next = Arc::new(next);

    assert_eq!(handle.apply(Arc::clone(&next)).ok(), Some(1));
    assert_eq!(handle.restart_pending(), vec!["runtime.poll_seconds"]);
    assert_eq!(handle.apply(next).ok(), Some(2));
    assert_eq!(handle.restart_pending(), vec!["runtime.poll_seconds"]);
}

#[tokio::test]
async fn a_profile_edit_applies_to_the_next_request() {
    let (manager, _cleanups, _directory) = manager().await;
    let handle = Arc::clone(&manager.inner.config);
    let first = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(first.allocation.size, Some(test_support::size()));

    let mut next = (*handle.current()).clone();
    for profile in next.profiles.values_mut() {
        profile.cpu_count = 2;
    }
    handle
        .apply(Arc::new(next))
        .unwrap_or_else(|error| unreachable!("apply: {error}"));

    // The allocation in flight keeps the policy it was authorized under.
    let in_flight = manager
        .get(first.allocation.id)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert_eq!(in_flight.size, Some(test_support::size()));

    manager
        .cancel_authorized(first.allocation.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    await_terminal(&manager, first.allocation.id).await;

    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    let second = manager
        .create(request, RequestOptions::default(), &claims)
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(
        second.allocation.size,
        Some(flanforge_core::GuestSize {
            cpu_count: 2,
            memory_mb: 8_192,
        })
    );
}

#[tokio::test]
async fn a_request_above_its_profile_ceiling_is_rejected_not_clamped() {
    let (manager, _cleanups, _directory) = manager().await;
    let result = manager
        .create(
            test_support::request(),
            RequestOptions {
                warm: false,
                cpu_count: Some(64),
                memory_mb: None,
            },
            &test_support::claims(),
        )
        .await;
    assert!(matches!(result, Err(ManagerError::RequestExceedsProfile)));
}
