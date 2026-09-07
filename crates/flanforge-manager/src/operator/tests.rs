use std::sync::Arc;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationState, CloneKind, CloneSource, Config,
    HotGuest, HotLane, HotState, Profile, RequestOptions, RunnerLabel, VmName,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, HotGuestStore, StoreError, WarmImageStore};
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, BusyReason, CleanupBudget,
    ConfigHandle, ManagerError, WorkerError,
};
use super::status::bounded_count;

use crate::tests::UnavailableWarmImageStore;
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

#[test]
fn wire_counts_saturate_at_the_contract_bound() {
    assert_eq!(bounded_count(12), 12);
    assert_eq!(bounded_count(usize::MAX), 65_535);
}

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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        if let Some(cleanups) = &self.cleanups {
            cleanups.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        if let Some(gate) = &self.cleanup_gate {
            let _permit = gate.acquire().await;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct UnavailableHostWorker;

#[async_trait]
impl AllocationWorker for UnavailableHostWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<super::super::HostMachine>, WorkerError> {
        Err(WorkerError::new("host inventory unavailable"))
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
        SqliteAllocationStore::open(&directory.join("state.db"))
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
    worker: impl AllocationWorker + 'static,
) -> AllocationManager {
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    manager_over_images(directory, store, config, worker, images).await
}

async fn manager_over_images(
    directory: &std::path::Path,
    store: Arc<dyn AllocationStore>,
    config: Arc<Config>,
    worker: impl AllocationWorker + 'static,
    images: Arc<dyn WarmImageStore>,
) -> AllocationManager {
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    AllocationManager::new(
        ConfigHandle::new(config),
        store,
        images,
        hot,
        Arc::new(worker),
    )
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

async fn await_cleanup_count(cleanups: &std::sync::atomic::AtomicUsize, expected: usize) {
    for _ in 0..500 {
        if cleanups.load(std::sync::atomic::Ordering::Acquire) == expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    unreachable!("cleanup count did not reach {expected}");
}

async fn await_receiver_count(manager: &AllocationManager, id: AllocationId, expected: usize) {
    for _ in 0..500 {
        let receivers = manager
            .inner
            .entries
            .lock()
            .await
            .get(&id)
            .map(|entry| entry.sender.receiver_count())
            .unwrap_or_default();
        if receivers == expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    unreachable!("receiver count did not reach {expected}");
}

async fn await_shutdown_start(manager: &AllocationManager) {
    for _ in 0..500 {
        if manager
            .inner
            .is_closing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    unreachable!("shutdown did not start");
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
async fn status_reports_when_warm_image_authority_is_unavailable() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manager = manager_over_images(
        directory.path(),
        store(directory.path()).await,
        test_support::config(directory.path().to_path_buf()),
        ParkedWorker::default(),
        Arc::new(UnavailableWarmImageStore),
    )
    .await;

    let status = manager.status_snapshot().await;
    assert!(!status.is_warm_image_store_visible);
    assert!(status.warm_images.is_empty());
}

/// RUN-745: the pool holds a slot continuously, and an operator must be able
/// to see that rather than read one active allocation against two slots and
/// conclude there is room.
#[tokio::test]
async fn status_reports_the_slot_an_idle_hot_guest_holds() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let name = "ci-project-9-1";
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let mut guest = HotGuest::new(
        VmName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        test_support::profile_name(),
        HotLane::Protected,
        test_support::size(),
        CloneSource {
            name: VmName::new("flanforge-base")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            kind: CloneKind::Template,
            base_fingerprint: None,
            fallback_reason: None,
        },
        None,
        None,
    );
    guest.state = HotState::Idle;
    hot.save(&guest)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut config = (*test_support::hot_config(directory.path().to_path_buf())).clone();
    config.runtime.max_running_vms = 2;
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::new(config)),
        store(directory.path()).await,
        Arc::new(
            SqliteWarmImageStore::open(&directory.path().join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::clone(&hot),
        Arc::new(PooledHostWorker(name.to_owned())),
    );

    let capacity = manager.status_snapshot().await.capacity;
    assert_eq!(capacity.hot_running, 1, "the pool's slot is reported");
    assert_eq!(capacity.max_hot_vms, 1);
    assert_eq!(capacity.max_running_vms, 2);
    // Not foreign: classifying it that way is what the record exists to stop.
    assert_eq!(capacity.foreign_running, 0);
    assert_eq!(capacity.active_allocations, 0);

    // And the same record is one row of the listing the operator can ask for.
    let listed = manager.hot_list().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].vm_name.as_str(), name);
    assert_eq!(listed[0].state, HotState::Idle);
    assert!(listed[0].is_machine_present);
}

/// A host reporting exactly one machine, the pool's, owned by this service.
#[derive(Debug)]
struct PooledHostWorker(String);

#[async_trait]
impl AllocationWorker for PooledHostWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<super::super::HostMachine>, WorkerError> {
        Ok(vec![super::super::HostMachine {
            name: self.0.clone(),
            state: super::super::MachineState::Running,
            age_seconds: Some(60),
            size: None,
            ownership: super::super::MachineOwnership::Owned,
        }])
    }
}

#[tokio::test]
async fn status_reports_when_host_inventory_is_unavailable() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manager = manager_over(
        directory.path(),
        store(directory.path()).await,
        test_support::config(directory.path().to_path_buf()),
        UnavailableHostWorker,
    )
    .await;

    let status = manager.status_snapshot().await;
    assert!(!status.capacity.is_host_visible);
    assert!(status.is_warm_image_store_visible);
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

/// RUN-384/RUN-582: the authorized path drives one ownerless cleanup and
/// concurrent callers receive the same terminal answer.
#[tokio::test]
async fn two_overlapping_authorized_cancels_clean_up_once() {
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

    let mut unauthorized = test_support::claims();
    unauthorized.repository = "owner/other".into();
    assert!(matches!(
        manager
            .cancel_authorized(interrupted.id, &unauthorized)
            .await,
        Err(ManagerError::Authorization(_))
    ));
    assert_eq!(cleanups.load(std::sync::atomic::Ordering::Acquire), 0);

    let first = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .cancel_authorized(interrupted.id, &test_support::claims())
                .await
        }
    });
    // Let the first call take ownership before the second arrives.
    await_cleanup_count(&cleanups, 1).await;
    let second = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .cancel_authorized(interrupted.id, &test_support::claims())
                .await
        }
    });
    await_receiver_count(&manager, interrupted.id, 1).await;
    let shutdown = tokio::spawn({
        let manager = manager.clone();
        async move { manager.shutdown(std::time::Duration::from_secs(30)).await }
    });
    await_shutdown_start(&manager).await;
    assert!(!shutdown.is_finished());
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
    assert!(
        shutdown
            .await
            .unwrap_or_else(|error| unreachable!("join: {error}"))
            .is_clean()
    );

    let repeated = manager
        .cancel_authorized(interrupted.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("repeat cancel: {error}"));
    assert_eq!(repeated.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(std::sync::atomic::Ordering::Acquire), 1);
}
