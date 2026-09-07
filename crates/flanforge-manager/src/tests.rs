use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationState, Profile, ProfileName, RequestOptions,
    WarmImageRecord,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, HotGuestStore, StoreError, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
    ManagerError, WorkerError,
    service::Entry,
    state::prune_terminal_history,
    tuning::{IN_MEMORY_ALLOCATION_LIMIT, ManagerTuning},
};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

#[derive(Debug, Default)]
pub(crate) struct UnavailableWarmImageStore;

#[async_trait]
impl WarmImageStore for UnavailableWarmImageStore {
    async fn load_all(&self) -> Result<Vec<WarmImageRecord>, StoreError> {
        Err(StoreError::Sync)
    }

    async fn load(&self, _profile: &ProfileName) -> Result<Option<WarmImageRecord>, StoreError> {
        Err(StoreError::Sync)
    }

    async fn save(&self, _record: &WarmImageRecord) -> Result<(), StoreError> {
        Err(StoreError::Sync)
    }

    async fn remove(&self, _profile: &ProfileName) -> Result<(), StoreError> {
        Err(StoreError::Sync)
    }
}

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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        let attempt = self.cleanups.fetch_add(1, Ordering::SeqCst);
        if attempt < self.cleanup_failures {
            return Err(WorkerError::new("cleanup failed"));
        }
        Ok(())
    }
}

/// The unwinding worker ARCH-245 is about: nothing in the task observes a
/// panic, so only a scoped guard can still run teardown.
#[derive(Debug)]
struct PanickingWorker {
    cleanups: Arc<AtomicUsize>,
}

#[async_trait]
impl AllocationWorker for PanickingWorker {
    #[expect(clippy::panic, reason = "the unwind this test injects")]
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        // Far enough in that a VM and a runner registration could exist.
        reporter
            .transition(AllocationState::Preparing)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        panic!("a dependency panicked inside the allocation worker");
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug)]
struct RejectFirstCleaningStore {
    inner: Arc<dyn AllocationStore>,
    has_rejected: AtomicBool,
}

#[async_trait]
impl AllocationStore for RejectFirstCleaningStore {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError> {
        self.inner.load_recent(limit).await
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError> {
        if allocation.state == AllocationState::Cleaning
            && !self.has_rejected.swap(true, Ordering::AcqRel)
        {
            return Err(StoreError::Sync);
        }
        self.inner.save(allocation).await
    }
}

#[derive(Debug)]
struct RejectAllocationStore {
    inner: Arc<dyn AllocationStore>,
    rejected: AllocationId,
}

#[async_trait]
impl AllocationStore for RejectAllocationStore {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError> {
        self.inner.load_recent(limit).await
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError> {
        if allocation.id == self.rejected {
            return Err(StoreError::Sync);
        }
        self.inner.save(allocation).await
    }
}

async fn images(directory: &tempfile::TempDir) -> Arc<dyn WarmImageStore> {
    Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

async fn hot(directory: &tempfile::TempDir) -> Arc<dyn HotGuestStore> {
    Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

async fn manager() -> (AllocationManager, Arc<AtomicUsize>, tempfile::TempDir) {
    manager_with_cleanup_failures(0).await
}

/// Captures every recorded event so a test can assert what the daemon would
/// have made durable.
#[derive(Debug, Default)]
struct RecordingEventSink {
    events: std::sync::Mutex<Vec<flanforge_store::Event>>,
}

#[async_trait::async_trait]
impl flanforge_store::EventSink for RecordingEventSink {
    async fn record(&self, event: flanforge_store::Event) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

#[tokio::test]
async fn lifecycle_facts_land_in_the_event_sink() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (config, store, images, hot) = dependencies(&directory).await;
    let sink = Arc::new(RecordingEventSink::default());
    let manager = AllocationManager::new_with_events(
        config,
        store,
        images,
        hot,
        sink.clone(),
        Arc::new(WaitingWorker::new(Arc::new(AtomicUsize::new(0)), 0)),
    );

    let created = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    manager
        .cancel_authorized(created.allocation.id, &test_support::claims())
        .await
        .unwrap_or_else(|error| unreachable!("cancel: {error}"));
    await_terminal(&manager, created.allocation.id).await;

    let events = sink
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let transitions: Vec<_> = events
        .iter()
        .filter(|event| event.kind == flanforge_store::EventKind::AllocationStateChanged)
        .collect();
    assert!(!transitions.is_empty(), "no transition events recorded");
    for event in &transitions {
        assert_eq!(event.allocation_id, Some(created.allocation.id));
        assert!(event.payload.as_deref().is_some_and(|p| p.contains("to")));
    }
}

async fn manager_with_cleanup_failures(
    cleanup_failures: usize,
) -> (AllocationManager, Arc<AtomicUsize>, tempfile::TempDir) {
    let cleanups = Arc::new(AtomicUsize::new(0));
    let (manager, directory) = manager_with_worker(Arc::new(WaitingWorker::new(
        cleanups.clone(),
        cleanup_failures,
    )))
    .await;
    (manager, cleanups, directory)
}

/// One tempdir's worth of durable dependencies, so a fixture and a test that
/// builds the manager itself seed the same state directory the same way.
async fn dependencies(
    directory: &tempfile::TempDir,
) -> (
    Arc<ConfigHandle>,
    Arc<dyn AllocationStore>,
    Arc<dyn WarmImageStore>,
    Arc<dyn HotGuestStore>,
) {
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    (
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(directory).await,
        hot(directory).await,
    )
}

async fn manager_with_worker(
    worker: Arc<dyn AllocationWorker>,
) -> (AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (config, store, images, hot) = dependencies(&directory).await;
    (
        AllocationManager::with_tuning(
            config,
            store,
            images,
            hot,
            worker,
            ManagerTuning::fast_cleanup_retries(),
        ),
        directory,
    )
}

/// A hang detector, not a budget: teardown signals the moment it lands, so
/// nothing correct ever waits this long.
const TERMINAL_WAIT: std::time::Duration = std::time::Duration::from_mins(1);

/// Waits on the allocation's own watch channel, the way shutdown and the
/// operator cancel path do. Polling against a wall clock fails a healthy
/// teardown whose durable writes are slow under a parallel test run.
async fn await_terminal(manager: &AllocationManager, id: AllocationId) -> Allocation {
    let Some(mut receiver) = manager
        .inner
        .entries
        .lock()
        .await
        .get(&id)
        .map(|entry| entry.sender.subscribe())
    else {
        unreachable!("no resident entry for {id}")
    };
    let settled = tokio::time::timeout(TERMINAL_WAIT, async {
        while !receiver.borrow_and_update().state.is_terminal() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    let allocation = manager
        .get(id)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    if settled.is_err() {
        unreachable!("allocation stalled in {:?}", allocation.state);
    }
    allocation
}

fn allocation_in_state(state: AllocationState, name: &str) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        flanforge_core::VmName::new(format!("ci-{name}"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::RunnerLabel::new(name)
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        flanforge_core::AllocationMode::Cold,
        test_support::size(),
    );
    allocation.state = state;
    allocation
}

async fn assert_terminal_predecessor_is_not_replayed(state: AllocationState, recover: bool) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let predecessor = allocation_in_state(state, "terminal-predecessor");
    store
        .save(&predecessor)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(&directory).await,
        hot(&directory).await,
        Arc::new(WaitingWorker::new(cleanups, 0)),
    );
    if recover {
        manager
            .recover()
            .await
            .unwrap_or_else(|error| unreachable!("recover: {error}"));
    } else {
        manager
            .ensure_entries_loaded(std::slice::from_ref(&predecessor))
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}"));
    }

    let created = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert!(created.is_new);
    assert_ne!(created.allocation.id, predecessor.id);
    assert!(
        manager
            .shutdown(std::time::Duration::from_secs(30))
            .await
            .is_clean()
    );
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
    let terminal = await_terminal(&manager, first.allocation.id).await;
    assert_eq!(terminal.state, AllocationState::Cancelled);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn completed_and_failed_predecessors_are_not_idempotent_matches() {
    assert_terminal_predecessor_is_not_replayed(AllocationState::Completed, false).await;
    assert_terminal_predecessor_is_not_replayed(AllocationState::Failed, false).await;
}

#[tokio::test]
async fn recovered_terminal_predecessor_is_not_an_idempotent_match() {
    assert_terminal_predecessor_is_not_replayed(AllocationState::Failed, true).await;
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
    assert!(
        manager
            .shutdown(std::time::Duration::from_secs(30))
            .await
            .is_clean()
    );
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

/// Seeds a durable record directly, at a fixed modification time so the
/// history window is the same set however fast the fixture writes.
/// Persists a copy stamped with the given update time, the column the
/// recency window orders by (the mtime's successor).
async fn seed_record(store: &SqliteAllocationStore, allocation: &Allocation, updated_at_unix: u64) {
    let mut record = allocation.clone();
    record.updated_at_unix = updated_at_unix;
    store
        .save(&record)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
}

/// Total persisted rows: history is bounded in memory, never deleted here.
async fn record_row_count(store: &SqliteAllocationStore) -> usize {
    store
        .load_recent(1_000_000)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"))
        .len()
}

/// Small enough to prove the working-set rules without seeding a
/// production-sized window; `IN_MEMORY_ALLOCATION_LIMIT` is pinned separately.
const TEST_HISTORY_LIMIT: usize = 8;

/// The fixtures shorten the cleanup retry pause, so this is where the values
/// `AllocationManager::new` actually ships with are pinned.
#[tokio::test]
async fn the_default_manager_uses_the_documented_history_bound_and_retry_pause() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (config, store, images, hot) = dependencies(&directory).await;
    let manager = AllocationManager::new(
        config,
        store,
        images,
        hot,
        Arc::new(WaitingWorker::new(Arc::new(AtomicUsize::new(0)), 0)),
    );

    // The literal is the bound ARCHITECTURE.md publishes; asserting only
    // against the constant lets the code and the document drift together.
    assert_eq!(IN_MEMORY_ALLOCATION_LIMIT, 1_000);
    assert_eq!(
        manager.inner.tuning.history_limit,
        IN_MEMORY_ALLOCATION_LIMIT
    );
    assert_eq!(
        manager.inner.tuning.cleanup_retry_delay,
        std::time::Duration::from_secs(1)
    );
}

#[tokio::test]
async fn recovery_keeps_active_work_inside_and_beyond_the_history_limit() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let aged_out = allocation_in_state(AllocationState::Running, "recovery-aged-out");
    seed_record(&store, &aged_out, 1).await;
    for index in 0..TEST_HISTORY_LIMIT {
        let allocation = allocation_in_state(AllocationState::Completed, &format!("old-{index}"));
        seed_record(&store, &allocation, 100 + index as u64).await;
    }
    let active = allocation_in_state(AllocationState::Preparing, "recovery-active");
    store
        .save(&active)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::with_tuning(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        Arc::clone(&store) as Arc<dyn AllocationStore>,
        images(&directory).await,
        hot(&directory).await,
        Arc::new(WaitingWorker::new(cleanups.clone(), 0)),
        ManagerTuning {
            history_limit: TEST_HISTORY_LIMIT,
            ..ManagerTuning::fast_cleanup_retries()
        },
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));

    // The newest window, plus the aged-out record that was still owed cleanup.
    assert_eq!(
        manager.inner.entries.lock().await.len(),
        TEST_HISTORY_LIMIT + 1
    );
    for allocation in [&active, &aged_out] {
        let recovered = manager
            .get(allocation.id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"));
        assert_eq!(recovered.state, AllocationState::Failed);
    }
    assert_eq!(cleanups.load(Ordering::SeqCst), 2);
    assert_eq!(record_row_count(&store).await, TEST_HISTORY_LIMIT + 2);
}

fn insert_entry(entries: &mut HashMap<AllocationId, Entry>, allocation: Allocation) {
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

#[test]
fn terminal_history_is_bounded_without_evicting_active_work() {
    let mut entries = HashMap::new();
    for index in 0..IN_MEMORY_ALLOCATION_LIMIT {
        let mut allocation =
            allocation_in_state(AllocationState::Completed, &format!("history-{index}"));
        allocation.updated_at_unix = index as u64;
        insert_entry(&mut entries, allocation);
    }
    // Oldest of them all, so only the terminal filter can save it.
    let mut active = allocation_in_state(AllocationState::Requested, "active");
    active.created_at_unix = 0;
    active.updated_at_unix = 0;
    let active_id = active.id;
    insert_entry(&mut entries, active);

    prune_terminal_history(&mut entries, IN_MEMORY_ALLOCATION_LIMIT);

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
    // This fixture's worker always fails cleanup, so the replacement cannot
    // tear down inside the grace and shutdown reports it rather than
    // claiming a clean stop.
    let report = manager.shutdown(std::time::Duration::from_secs(30)).await;
    assert_eq!(report.leaked().len(), 1, "{:?}", report.leaked());
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

/// ARCH-245: an unwinding worker must still tear its VM and runner down and
/// terminalize, or every later create answers Busy until a restart.
#[tokio::test]
async fn a_panicking_worker_still_cleans_up_and_frees_capacity() {
    let cleanups = Arc::new(AtomicUsize::new(0));
    let (manager, _directory) = manager_with_worker(Arc::new(PanickingWorker {
        cleanups: cleanups.clone(),
    }))
    .await;
    let first = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;

    let terminal = await_terminal(&manager, first.id).await;
    assert_eq!(terminal.state, AllocationState::Failed);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert!(
        terminal
            .error
            .is_some_and(|error| error.contains("without returning"))
    );

    let mut request = test_support::request();
    request.run_id = 43;
    let mut claims = test_support::claims();
    claims.run_id = "43".into();
    let second = manager
        .create(request, RequestOptions::default(), &claims)
        .await
        .unwrap_or_else(|error| unreachable!("create after the panic: {error}"));
    assert!(second.is_new);
    // The guard also terminalizes the replacement, so shutdown does not wait
    // out its budget on a task that unwound.
    assert!(
        manager
            .shutdown(std::time::Duration::from_secs(30))
            .await
            .is_clean()
    );
}

#[tokio::test]
async fn a_transient_cleaning_write_failure_does_not_skip_cleanup() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let inner: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let store = Arc::new(RejectFirstCleaningStore {
        inner,
        has_rejected: AtomicBool::new(false),
    });
    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(&directory).await,
        hot(&directory).await,
        Arc::new(WaitingWorker::new(cleanups.clone(), 0)),
    );
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
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn recovery_continues_after_a_record_cannot_be_persisted() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let inner: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let rejected = allocation_in_state(AllocationState::Booting, "rejected-record");
    let mut recoverable = allocation_in_state(AllocationState::Preparing, "recoverable-record");
    recoverable.request.run_id = 43;
    for allocation in [&rejected, &recoverable] {
        inner
            .save(allocation)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    let store: Arc<dyn AllocationStore> = Arc::new(RejectAllocationStore {
        inner,
        rejected: rejected.id,
    });
    let cleanups = Arc::new(AtomicUsize::new(0));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images(&directory).await,
        hot(&directory).await,
        Arc::new(WaitingWorker::new(cleanups.clone(), 0)),
    );

    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));

    assert_eq!(cleanups.load(Ordering::SeqCst), 2);
    assert!(
        !manager
            .get(rejected.id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"))
            .state
            .is_terminal()
    );
    assert_eq!(
        manager
            .get(recoverable.id)
            .await
            .unwrap_or_else(|error| unreachable!("get: {error}"))
            .state,
        AllocationState::Failed
    );

    assert!(matches!(
        manager
            .cancel_authorized(rejected.id, &test_support::claims())
            .await,
        Err(ManagerError::Store(StoreError::Sync))
    ));
    assert_eq!(cleanups.load(Ordering::SeqCst), 3);
    assert!(
        manager
            .shutdown(std::time::Duration::from_millis(100))
            .await
            .is_clean()
    );
}

#[tokio::test]
async fn recovery_reconciles_every_record_when_one_profile_is_missing() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
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
        hot(&directory).await,
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
            storage_mb: 40_960,
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
                hot: flanforge_core::HotRequest::Untouched,
                cpu_count: Some(64),
                memory_mb: None,
            },
            &test_support::claims(),
        )
        .await;
    assert!(matches!(result, Err(ManagerError::RequestExceedsProfile)));
}
