use std::sync::Arc;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationState, Config, Profile, RequestOptions,
    RunnerLabel, VmName,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, StoreError};
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, BusyReason, ConfigHandle,
    ManagerError, WorkerError,
};

/// Parks in `waiting_for_job`, exactly like a workflow whose dependent job was
/// cancelled before it queued.
#[derive(Debug, Default)]
struct ParkedWorker {
    /// Cleanup waits on this, so a test can hold recovery mid-flight.
    cleanup_gate: Option<Arc<Semaphore>>,
    /// Counts cleanup entries, so a duplicated teardown is visible.
    cleanups: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

#[async_trait]
impl AllocationWorker for ParkedWorker {
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
        if let Some(cleanups) = &self.cleanups {
            cleanups.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        if let Some(gate) = &self.cleanup_gate {
            let _permit = gate.acquire().await;
        }
        Ok(())
    }
}

/// Stops accepting writes on demand, so the worker task ends with its entry
/// still non-terminal — the abandoned-entry shape a store failure produces.
#[derive(Debug)]
struct FailingStore {
    inner: Arc<dyn AllocationStore>,
    is_failing: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AllocationStore for FailingStore {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError> {
        self.inner.load_recent(limit).await
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError> {
        if self.is_failing.load(std::sync::atomic::Ordering::Acquire) {
            return Err(StoreError::Sync);
        }
        self.inner.save(allocation).await
    }
}

async fn store(directory: &std::path::Path) -> Arc<dyn AllocationStore> {
    Arc::new(
        JsonStateStore::open(directory)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

/// The state store holds an exclusive lock, so a test that seeds records keeps
/// the one instance it opened.
async fn manager_over(
    directory: &std::path::Path,
    store: Arc<dyn AllocationStore>,
    config: Arc<Config>,
    worker: ParkedWorker,
) -> AllocationManager {
    let images = Arc::new(
        JsonWarmImageStore::open(directory)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    AllocationManager::new(ConfigHandle::new(config), store, images, Arc::new(worker))
}

fn budgeted(state_dir: std::path::PathBuf) -> Arc<Config> {
    let mut config = (*test_support::config(state_dir)).clone();
    config.runtime.host_cpu_count = Some(4);
    config.runtime.host_memory_mb = Some(8_192);
    Arc::new(config)
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
async fn cancelling_a_waiting_for_job_allocation_reaches_terminal_and_frees_capacity() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manager = manager_over(
        directory.path(),
        store(directory.path()).await,
        budgeted(directory.path().to_path_buf()),
        ParkedWorker::default(),
    )
    .await;
    let stuck = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;

    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    assert!(matches!(
        manager
            .create(request.clone(), RequestOptions::default(), &claims)
            .await,
        Err(ManagerError::Busy {
            reason: BusyReason::Budget,
            ..
        })
    ));

    let summary = manager.list().await;
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].id, stuck.id);

    manager
        .cancel_by_id(stuck.id)
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    assert_eq!(
        await_terminal(&manager, stuck.id).await.state,
        AllocationState::Cancelled
    );
    assert!(
        manager
            .create(request, RequestOptions::default(), &claims)
            .await
            .is_ok(),
        "the freed slot admits the next request"
    );
}

#[tokio::test]
async fn cancelling_an_unsupervised_recovered_allocation_reaches_terminal() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = store(directory.path()).await;
    let mut interrupted = Allocation::new(
        test_support::request(),
        VmName::new("ci-unsupervised").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("unsupervised").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    interrupted.state = AllocationState::WaitingForJob;
    store
        .save(&interrupted)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // Recovery is not run, so the entry exists with nothing listening to it.
    let manager = manager_over(
        directory.path(),
        store,
        test_support::config(directory.path().to_path_buf()),
        ParkedWorker::default(),
    )
    .await;
    manager
        .ensure_entries_loaded(std::slice::from_ref(&interrupted))
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));

    let cancelled = manager
        .cancel_by_id(interrupted.id)
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    assert_eq!(cancelled.state, AllocationState::Cancelled);
}

#[tokio::test]
async fn an_unknown_id_is_not_found() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manager = manager_over(
        directory.path(),
        store(directory.path()).await,
        test_support::config(directory.path().to_path_buf()),
        ParkedWorker::default(),
    )
    .await;
    assert!(matches!(
        manager.cancel_by_id(AllocationId::new()).await,
        Err(ManagerError::NotFound(_))
    ));
}

#[tokio::test]
async fn recovery_rebuilds_the_committed_total_from_durable_records() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = store(directory.path()).await;
    let mut interrupted = Allocation::new(
        test_support::request(),
        VmName::new("ci-interrupted").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("interrupted").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    interrupted.state = AllocationState::Ready;
    store
        .save(&interrupted)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let gate = Arc::new(Semaphore::new(0));
    let manager = manager_over(
        directory.path(),
        store,
        budgeted(directory.path().to_path_buf()),
        ParkedWorker {
            cleanup_gate: Some(Arc::clone(&gate)),
            cleanups: None,
        },
    )
    .await;
    let recovering = tokio::spawn({
        let manager = manager.clone();
        async move { manager.recover().await }
    });

    // The restart must charge the durable record before anything releases it.
    for _ in 0..500 {
        if manager.status_snapshot().await.capacity.active_allocations == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let snapshot = manager.status_snapshot().await;
    assert_eq!(snapshot.capacity.active_allocations, 1);
    assert_eq!(snapshot.capacity.committed_cpu_count, 4);
    assert_eq!(snapshot.capacity.committed_memory_mb, 8_192);
    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    assert!(matches!(
        manager
            .create(request.clone(), RequestOptions::default(), &claims)
            .await,
        Err(ManagerError::Busy { .. })
    ));

    gate.add_permits(1);
    recovering
        .await
        .unwrap_or_else(|error| unreachable!("join: {error}"))
        .unwrap_or_else(|error| unreachable!("recover: {error}"));
    assert_eq!(
        manager.status_snapshot().await.capacity.active_allocations,
        0
    );
    assert!(
        manager
            .create(request, RequestOptions::default(), &claims)
            .await
            .is_ok()
    );
}

/// RUN-560: supervision is a fact about a running task, not a creation flag, so
/// a cancel after the task gave up must actually terminalize the entry.
#[tokio::test]
async fn cancelling_an_entry_whose_worker_gave_up_reaches_terminal_and_frees_capacity() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(FailingStore {
        inner: store(directory.path()).await,
        is_failing: std::sync::atomic::AtomicBool::new(false),
    });
    let manager = manager_over(
        directory.path(),
        Arc::clone(&store) as Arc<dyn AllocationStore>,
        budgeted(directory.path().to_path_buf()),
        ParkedWorker::default(),
    )
    .await;
    let abandoned = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;

    // A supervised entry is only signalled, and the worker still owns it.
    let signalled = manager
        .cancel_by_id(abandoned.id)
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    assert!(!signalled.state.is_terminal());

    // The worker's own terminalization now fails, so it exits abandoning it.
    store
        .is_failing
        .store(true, std::sync::atomic::Ordering::Release);
    for _ in 0..500 {
        if !manager.is_supervised(abandoned.id).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!manager.is_supervised(abandoned.id).await);
    assert!(
        !manager
            .get(abandoned.id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"))
            .state
            .is_terminal()
    );

    store
        .is_failing
        .store(false, std::sync::atomic::Ordering::Release);
    let cancelled = manager
        .cancel_by_id(abandoned.id)
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    assert_eq!(cancelled.state, AllocationState::Cancelled);

    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    assert!(
        manager
            .create(request, RequestOptions::default(), &claims)
            .await
            .is_ok(),
        "the released entry frees the budget"
    );
}

/// RUN-582: two overlapping operator cancels run one cleanup and answer alike.
#[tokio::test]
async fn two_overlapping_cancels_clean_up_once() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = store(directory.path()).await;
    let mut interrupted = Allocation::new(
        test_support::request(),
        VmName::new("ci-unsupervised").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("unsupervised").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    interrupted.state = AllocationState::WaitingForJob;
    store
        .save(&interrupted)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let gate = Arc::new(Semaphore::new(0));
    let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let manager = manager_over(
        directory.path(),
        store,
        test_support::config(directory.path().to_path_buf()),
        ParkedWorker {
            cleanup_gate: Some(Arc::clone(&gate)),
            cleanups: Some(Arc::clone(&cleanups)),
        },
    )
    .await;
    manager
        .ensure_entries_loaded(std::slice::from_ref(&interrupted))
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));

    let first = tokio::spawn({
        let manager = manager.clone();
        async move { manager.cancel_by_id(interrupted.id).await }
    });
    // Let the first call take ownership before the second arrives.
    for _ in 0..500 {
        if cleanups.load(std::sync::atomic::Ordering::Acquire) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let second = tokio::spawn({
        let manager = manager.clone();
        async move { manager.cancel_by_id(interrupted.id).await }
    });
    gate.add_permits(2);

    let first = first
        .await
        .unwrap_or_else(|error| unreachable!("join: {error}"))
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    let second = second
        .await
        .unwrap_or_else(|error| unreachable!("join: {error}"))
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    assert_eq!(first.state, AllocationState::Cancelled);
    assert_eq!(second.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(first.error, None);
}
