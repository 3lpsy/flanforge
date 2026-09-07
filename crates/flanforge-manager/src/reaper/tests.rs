use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource, Config,
    HotGuest, HotLane, HotState, Profile, RunnerLabel, VmName, WarmImageRecord, WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, HotGuestStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    super::{
        AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
        HostMachine, MachineOwnership, MachineState, ReapRequest, WorkerError,
        tuning::ManagerTuning,
    },
    ReapAuthorization, ReapInputs, plan_sweep, unaged_candidates,
};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

/// The prefix every fixture configuration delegates to the daemon.
const PREFIX: &str = "ci-";

#[derive(Debug)]
struct HostWorker {
    /// A deleted machine stops being listed, so a later pass — and the capacity
    /// probe — see what the sweep actually did.
    machines: Mutex<Vec<HostMachine>>,
    deleted: Mutex<Vec<String>>,
    /// Applied on the first deletion, so a test can reload configuration in the
    /// middle of a sweep exactly as the watcher would.
    reload: Mutex<Option<(Arc<ConfigHandle>, Arc<Config>)>>,
    /// Fails every teardown, so a test can exhaust cleanup the way RUN-102 did.
    is_cleanup_exhausted: bool,
}

impl HostWorker {
    fn new(machines: Vec<HostMachine>) -> Self {
        Self {
            machines: Mutex::new(machines),
            deleted: Mutex::new(Vec::new()),
            reload: Mutex::new(None),
            is_cleanup_exhausted: false,
        }
    }

    fn exhausting(machines: Vec<HostMachine>) -> Self {
        Self {
            is_cleanup_exhausted: true,
            ..Self::new(machines)
        }
    }

    fn reloading(machines: Vec<HostMachine>, handle: Arc<ConfigHandle>, next: Arc<Config>) -> Self {
        Self {
            reload: Mutex::new(Some((handle, next))),
            ..Self::new(machines)
        }
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted
            .lock()
            .map(|deleted| deleted.clone())
            .unwrap_or_default()
    }

    fn listed(&self) -> Vec<HostMachine> {
        self.machines
            .lock()
            .map(|machines| machines.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl AllocationWorker for HostWorker {
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
        if self.is_cleanup_exhausted {
            return Err(WorkerError::new("cleanup exhausted its retries"));
        }
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(self.listed())
    }

    /// Mirrors both backends' re-check: the prefix authorizes a name inside the
    /// boundary and nothing outside it, and live configuration is refused.
    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        if request.authorization == &ReapAuthorization::Prefix
            && !request.name.as_str().starts_with(PREFIX)
        {
            return Err(WorkerError::new("refusing to delete an unowned VM"));
        }
        if request.reserved.contains(request.name.as_str()) {
            return Err(WorkerError::new("live configuration claims that name"));
        }
        if let Ok(mut deleted) = self.deleted.lock() {
            deleted.push(request.name.to_string());
        }
        if let Ok(mut machines) = self.machines.lock() {
            machines.retain(|machine| machine.name != request.name.as_str());
        }
        if let Ok(mut reload) = self.reload.lock()
            && let Some((handle, next)) = reload.take()
        {
            let _ = handle.apply(next);
        }
        Ok(())
    }
}

/// A store whose history window has dropped everything, which is what an
/// orphan older than `IN_MEMORY_ALLOCATION_LIMIT` allocations looks like to the
/// process that restarts after it.
#[derive(Debug)]
struct AgedOutStore(Arc<dyn AllocationStore>);

#[async_trait]
impl AllocationStore for AgedOutStore {
    async fn load_recent(
        &self,
        _limit: usize,
    ) -> Result<Vec<Allocation>, flanforge_store::StoreError> {
        Ok(Vec::new())
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), flanforge_store::StoreError> {
        self.0.save(allocation).await
    }
}

fn machine(name: &str, age_seconds: u64) -> HostMachine {
    HostMachine {
        name: name.to_owned(),
        state: MachineState::Stopped,
        age_seconds: Some(age_seconds),
        size: None,
        // The name-addressed rule: the configured prefix is the only ownership
        // evidence a backend without per-VM metadata has.
        ownership: if name.starts_with(PREFIX) {
            MachineOwnership::Owned
        } else {
            MachineOwnership::Foreign
        },
    }
}

fn running(name: &str, age_seconds: u64) -> HostMachine {
    HostMachine {
        state: MachineState::Running,
        ..machine(name, age_seconds)
    }
}

fn unaged(name: &str) -> HostMachine {
    HostMachine {
        age_seconds: None,
        ..machine(name, 0)
    }
}

fn allocation(vm_name: &str, state: AllocationState, mode: AllocationMode) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new(vm_name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-sweep")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        mode,
        test_support::size(),
    );
    allocation.state = state;
    allocation.vm_created = true;
    allocation
}

fn record(warm_template: &str, generation: u64) -> WarmImageRecord {
    WarmImageRecord {
        profile: test_support::profile_name(),
        warm_template: VmName::new(warm_template)
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        generation,
        base_fingerprint: BaseFingerprint::new("aa01")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1_755_324_251,
        state: WarmImageState::Promoted,
        previous: None,
    }
}

const DAY: u64 = 90_000;

#[test]
fn plan_sweep_skips_referenced_images_active_allocations_and_non_prefix_names() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let machines = [
        machine("flanforge-base", DAY),
        machine("project-warm", DAY),
        machine("project-warm.previous", DAY),
        machine("someones-laptop", DAY),
        machine("ci-project-42-1", DAY),
    ];
    let allocations = [allocation(
        "ci-project-42-1",
        AllocationState::Running,
        AllocationMode::Cold,
    )];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &allocations,
        images: &[record("project-warm", 3)],
        hot: &[],
    });
    assert!(planned.is_empty(), "{planned:?}");
}

/// RUN-102: the configured prefix is the ownership boundary, so a prefixed
/// name no record and no live allocation claims is the daemon's own orphan.
#[test]
fn plan_sweep_authorizes_a_prefixed_name_no_record_claims() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [machine("ci-unknown-9-1", DAY)];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[],
        hot: &[],
    });
    assert_eq!(planned.len(), 1);
    assert_eq!(
        planned.first().map(|candidate| &candidate.authorization),
        Some(&ReapAuthorization::Prefix)
    );
}

/// RUN-102: the prefix authorizes nothing outside itself. A name no record
/// claims and no prefix covers is not the daemon's to collect.
#[test]
fn a_non_prefixed_name_with_no_record_is_never_a_candidate() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [
        machine("someones-laptop", DAY),
        machine("devboxvm", DAY),
        machine("flanforge-base", DAY),
    ];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[],
        hot: &[],
    });
    assert!(planned.is_empty(), "{planned:?}");
}

/// RUN-102: an active allocation still holds its prefixed name, whatever the
/// prefix would otherwise authorize.
#[test]
fn a_prefixed_name_an_active_allocation_holds_is_never_a_candidate() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [machine("ci-project-42-1", DAY)];

    for state in [
        AllocationState::Requested,
        AllocationState::Preparing,
        AllocationState::Booting,
        AllocationState::Registering,
        AllocationState::WaitingForJob,
        AllocationState::Ready,
        AllocationState::Running,
        AllocationState::Cleaning,
    ] {
        let held = [allocation("ci-project-42-1", state, AllocationMode::Cold)];
        let planned = plan_sweep(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &held,
            images: &[],
            hot: &[],
        });
        assert!(planned.is_empty(), "{state:?}: {planned:?}");
    }
}

#[test]
fn the_reaper_still_collects_an_orphan_while_a_non_terminal_allocation_exists() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let orphan = allocation(
        "ci-project-41-1",
        AllocationState::Failed,
        AllocationMode::Cold,
    );
    let active = allocation(
        "ci-project-42-1",
        AllocationState::Booting,
        AllocationMode::Cold,
    );
    let machines = [
        machine("ci-project-41-1", DAY),
        machine("ci-project-42-1", DAY),
    ];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[orphan.clone(), active],
        images: &[],
        hot: &[],
    });
    assert_eq!(planned.len(), 1);
    let candidate = planned.first().unwrap_or_else(|| unreachable!("candidate"));
    assert_eq!(candidate.name, "ci-project-41-1");
    assert_eq!(
        candidate.authorization,
        ReapAuthorization::Record(orphan.id)
    );
}

#[test]
fn a_regeneration_in_flight_protects_all_three_of_its_names() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let machines = [machine("project-warm.staging", DAY)];
    let regenerating = [allocation(
        "ci-project-42-1",
        AllocationState::Running,
        AllocationMode::Regenerate,
    )];

    assert!(
        plan_sweep(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &regenerating,
            images: &[],
            hot: &[],
        })
        .is_empty()
    );
}

#[test]
fn reap_false_preserves_images_but_not_an_orphaned_clone() {
    let mut config = (*test_support::warm_config("/tmp/flanforge-sweep".into())).clone();
    for profile in config.profiles.values_mut() {
        profile.reap = false;
    }
    let orphan = allocation(
        "ci-project-41-1",
        AllocationState::Completed,
        AllocationMode::Cold,
    );
    let machines = [
        machine("project-warm.staging", DAY),
        machine("project-retired", DAY),
        machine("ci-project-41-1", DAY),
    ];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: std::slice::from_ref(&orphan),
        images: &[record("project-retired", 2)],
        hot: &[],
    });
    assert_eq!(
        planned
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ci-project-41-1"]
    );
}

#[test]
fn an_image_a_repointed_profile_left_behind_is_collected() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let machines = [
        machine("project-retired", DAY),
        machine("project-fresh", 4_000),
    ];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[record("project-retired", 2)],
        hot: &[],
    });
    assert_eq!(planned.len(), 1);
    assert_eq!(
        planned.first().map(|candidate| &candidate.authorization),
        Some(&ReapAuthorization::Image(test_support::profile_name()))
    );
}

/// The minimum-age gate is what keeps a clone being created right now out of
/// the sweep, so a name it cannot age is skipped even when the prefix owns it.
#[test]
fn a_candidate_below_the_age_floor_or_without_an_age_is_skipped() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [machine("ci-project-41-1", 60), unaged("ci-project-40-1")];

    assert!(
        plan_sweep(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &[],
            images: &[],
            hot: &[],
        })
        .is_empty()
    );
}

/// RUN-102: skipping is only safe while it is visible, so a prefix-owned name
/// the sweep cannot age is named. A name the prefix does not cover, and one an
/// active allocation holds, are not the sweep's to report.
#[test]
fn a_prefix_owned_name_the_sweep_cannot_age_is_reported() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [
        unaged("ci-project-40-1"),
        unaged("ci-project-42-1"),
        unaged("someones-laptop"),
        machine("ci-project-41-1", DAY),
    ];
    let held = [allocation(
        "ci-project-42-1",
        AllocationState::Running,
        AllocationMode::Cold,
    )];

    assert_eq!(
        super::unaged_candidates(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &held,
            images: &[],
            hot: &[],
        }),
        vec!["ci-project-40-1".to_owned()]
    );
}

async fn manager(
    config: Arc<Config>,
    machines: Vec<HostMachine>,
) -> (AllocationManager, Arc<HostWorker>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;
    let worker = Arc::new(HostWorker::new(machines));
    (
        AllocationManager::new(
            ConfigHandle::new(config),
            store,
            images,
            hot,
            worker.clone(),
        ),
        worker,
        directory,
    )
}

async fn hot_store(directory: &std::path::Path) -> Arc<dyn HotGuestStore> {
    Arc::new(
        SqliteHotGuestStore::open(&directory.join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )
}

#[tokio::test]
async fn a_dry_run_reports_without_deleting() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let (manager, worker, _directory) =
        manager(config, vec![machine("ci-project-41-1", DAY)]).await;

    let report = manager
        .sweep(true)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.planned.len(), 1);
    assert!(report.deleted.is_empty());
    assert!(worker.deleted().is_empty());
    // RUN-542: a dry run never replaces the record of the last deleting pass.
    assert_eq!(manager.last_sweep().await, None);
}

/// RUN-102: a prefixed name whose record has aged out of the record window is
/// still the daemon's, and is collected once past the age gate; a name outside
/// the prefix that nothing records is refused by the backend and reported.
#[tokio::test]
async fn a_sweep_collects_an_orphan_with_no_record_and_leaves_a_foreign_name() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let (manager, worker, _directory) = manager(
        config,
        vec![
            machine("ci-project-41-1", DAY),
            machine("someones-laptop", DAY),
        ],
    )
    .await;

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.deleted, vec!["ci-project-41-1".to_owned()]);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(worker.deleted(), vec!["ci-project-41-1".to_owned()]);
    assert_eq!(
        report
            .planned
            .first()
            .map(|candidate| &candidate.authorization),
        Some(&ReapAuthorization::Prefix)
    );
}

#[tokio::test]
async fn a_sweep_deletes_a_recorded_orphan_and_a_prefix_only_name() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let orphan = allocation(
        "ci-project-41-1",
        AllocationState::Failed,
        AllocationMode::Cold,
    );
    store
        .save(&orphan)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;
    let worker = Arc::new(HostWorker::new(vec![
        machine("ci-project-41-1", DAY),
        machine("ci-stranger-1-1", DAY),
    ]));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images,
        hot,
        worker.clone(),
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(
        report.deleted,
        vec!["ci-project-41-1".to_owned(), "ci-stranger-1-1".to_owned()]
    );
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(
        worker.deleted(),
        vec!["ci-project-41-1".to_owned(), "ci-stranger-1-1".to_owned()]
    );
    // The record's own name is authorized by the record, not by the prefix.
    let authorizations = report
        .planned
        .iter()
        .map(|candidate| candidate.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        authorizations,
        vec![
            ReapAuthorization::Record(orphan.id),
            ReapAuthorization::Prefix
        ]
    );
}

/// RUN-102: cleanup exhausts with the clone still present, so `ensure_terminal`
/// records `Failed` and nothing revisits that allocation. The running orphan
/// holds the host's only slot; a restart's sweep must give it back — including
/// once the record itself has aged out of the history window, which is the case
/// the reaper used to warn about forever and never collect.
#[tokio::test]
async fn an_orphan_left_by_exhausted_cleanup_is_collected_after_a_restart() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.max_running_vms = 1;
    // A fresh listing per question, so the sweep's effect on capacity is visible.
    config.runtime.poll_seconds = 0;
    let config = Arc::new(config);
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let interrupted = allocation(
        "ci-project-41-1",
        AllocationState::Cleaning,
        AllocationMode::Cold,
    );
    store
        .save(&interrupted)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let worker = Arc::new(HostWorker::exhausting(vec![running(
        "ci-project-41-1",
        DAY,
    )]));
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;

    let manager = AllocationManager::with_tuning(
        ConfigHandle::new(Arc::clone(&config)),
        Arc::clone(&store),
        Arc::clone(&images),
        Arc::clone(&hot),
        Arc::clone(&worker) as Arc<dyn AllocationWorker>,
        ManagerTuning::fast_cleanup_retries(),
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));
    let recovered = manager
        .get(interrupted.id)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert!(recovered.state.is_terminal(), "{:?}", recovered.state);
    assert!(
        worker.deleted().is_empty(),
        "cleanup exhausted, so the clone survives"
    );
    assert!(
        manager
            .create(
                test_support::request(),
                flanforge_core::RequestOptions::default(),
                &test_support::claims(),
            )
            .await
            .is_err(),
        "a running orphan takes the host's only slot"
    );
    assert!(
        manager
            .shutdown(std::time::Duration::from_secs(5))
            .await
            .is_clean()
    );

    // Restart: the same host, a new process, and a record that has aged out.
    let restarted = AllocationManager::new(
        ConfigHandle::new(config),
        Arc::new(AgedOutStore(store)) as Arc<dyn AllocationStore>,
        images,
        hot,
        Arc::clone(&worker) as Arc<dyn AllocationWorker>,
    );
    restarted
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));
    let report = restarted
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.deleted, vec!["ci-project-41-1".to_owned()]);
    assert!(worker.listed().is_empty(), "{:?}", worker.listed());
    assert!(
        restarted
            .create(
                test_support::request(),
                flanforge_core::RequestOptions::default(),
                &test_support::claims(),
            )
            .await
            .is_ok(),
        "the reclaimed slot is admissible again"
    );
    // The same failing worker backs the reclaimed slot, so its teardown is a
    // reported leak rather than a clean stop.
    let report = restarted.shutdown(std::time::Duration::from_secs(5)).await;
    assert_eq!(report.leaked().len(), 1, "{:?}", report.leaked());
}

/// A manager whose warm image store already holds `records`.
async fn manager_with_records(
    config: Arc<Config>,
    worker: Arc<HostWorker>,
    records: &[WarmImageRecord],
) -> (AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;
    for record in records {
        images
            .save(record)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    (
        AllocationManager::new(ConfigHandle::new(config), store, images, hot, worker),
        directory,
    )
}

/// The fixture configuration with the record's profile removed entirely.
fn config_without_the_recorded_profile(state_dir: std::path::PathBuf) -> Arc<Config> {
    let mut config = (*test_support::config(state_dir)).clone();
    let name = flanforge_core::ProfileName::new("other")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    config.profiles = std::collections::BTreeMap::from([(name, test_support::profile())]);
    Arc::new(config)
}

/// RUN-540: a warm image whose profile was removed is collected, not refused
/// every cycle, and its record is dropped once its images are gone.
#[tokio::test]
async fn a_removed_profile_leaves_an_image_the_sweep_can_actually_delete() {
    let config = config_without_the_recorded_profile("/tmp/flanforge-sweep".into());
    let worker = Arc::new(HostWorker::new(vec![machine("project-retired", DAY)]));
    let stored = record("project-retired", 2);
    let (manager, _directory) =
        manager_with_records(Arc::clone(&config), Arc::clone(&worker), &[stored]).await;

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.deleted, vec!["project-retired".to_owned()]);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(worker.deleted(), vec!["project-retired".to_owned()]);

    // The image is gone from the host on the next pass, so the record follows.
    let worker = Arc::new(HostWorker::new(Vec::new()));
    let stored = record("project-retired", 2);
    let (manager, _directory) = manager_with_records(config, worker, &[stored]).await;
    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.pruned_records, vec![test_support::profile_name()]);
}

/// RUN-543: a pruned record is reported, and a referenced one is left alone.
#[tokio::test]
async fn a_referenced_record_is_never_pruned_or_reported() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let worker = Arc::new(HostWorker::new(Vec::new()));
    let stored = record("project-warm", 3);
    let (manager, _directory) = manager_with_records(config, worker, &[stored]).await;

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert!(
        report.pruned_records.is_empty(),
        "{:?}",
        report.pruned_records
    );
    assert!(
        manager
            .status_snapshot()
            .await
            .warm_images
            .iter()
            .any(|image| image.profile == test_support::profile_name())
    );
}

/// RUN-541: a configuration claim made while the sweep runs is seen by the
/// re-check, which re-reads the current document rather than a pinned snapshot.
#[tokio::test]
async fn a_name_claimed_by_a_reload_mid_sweep_is_left_alone() {
    let config = config_without_the_recorded_profile("/tmp/flanforge-sweep".into());
    // The reload declares the second candidate as a profile's warm template.
    let mut claimed = (*config).clone();
    for profile in claimed.profiles.values_mut() {
        profile.warm_template = Some(
            VmName::new("project-claimed").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        profile.regeneration_workflow = Some("apple.yml".to_owned());
    }
    claimed
        .runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
        .home = Some("/opt/flanforge/tart".into());
    let handle = ConfigHandle::new(Arc::clone(&config));
    let worker = Arc::new(HostWorker::reloading(
        vec![
            machine("project-retired", DAY),
            machine("project-claimed", DAY),
        ],
        Arc::clone(&handle),
        Arc::new(claimed),
    ));
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;
    for stored in [record("project-retired", 2), claimed_record()] {
        images
            .save(&stored)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    let manager = AllocationManager::new(handle, store, images, hot, worker.clone());

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.deleted, vec!["project-retired".to_owned()]);
    assert_eq!(report.skipped, vec!["project-claimed".to_owned()]);
    assert!(
        !worker.deleted().contains(&"project-claimed".to_owned()),
        "{:?}",
        worker.deleted()
    );
}

/// CORE-505: an inert pass says so, rather than reading as a clean host.
#[tokio::test]
async fn a_pass_that_cannot_age_anything_reports_why_it_planned_nothing() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let worker = Arc::new(HostWorker::new(vec![HostMachine {
        name: "ci-project-41-1".to_owned(),
        state: MachineState::Stopped,
        age_seconds: None,
        size: None,
        ownership: MachineOwnership::Unknown,
    }]));
    let (manager, _directory) = manager_with_records(config, worker, &[]).await;

    let report = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert!(report.planned.is_empty());
    assert_eq!(
        report.inert_reason,
        Some(super::SweepInertReason::AgeUndeterminable)
    );
}

fn claimed_record() -> WarmImageRecord {
    WarmImageRecord {
        profile: flanforge_core::ProfileName::new("other")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ..record("project-claimed", 1)
    }
}

/// A backend that retires pointer-addressed generations, recording what the
/// manager told it so the prune-then-sweep ordering is observable.
#[derive(Debug, Default)]
struct SweepingWorker {
    seen: Mutex<Vec<SeenSweep>>,
}

#[derive(Clone, Debug)]
struct SeenSweep {
    is_dry_run: bool,
    declared: std::collections::BTreeMap<flanforge_core::ProfileName, VmName>,
    claimed: std::collections::BTreeSet<flanforge_core::ProfileName>,
}

impl SweepingWorker {
    fn seen(&self) -> Vec<SeenSweep> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl AllocationWorker for SweepingWorker {
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

    async fn sweep_images(
        &self,
        request: crate::ImageSweep<'_>,
    ) -> Result<Vec<crate::RetiredImage>, WorkerError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(SeenSweep {
                is_dry_run: request.is_dry_run,
                declared: request.declared.clone(),
                claimed: request.claimed.clone(),
            });
        }
        Ok(vec![crate::RetiredImage {
            profile: test_support::profile_name(),
            generation: 1,
            age_seconds: 7_200,
            is_deleted: !request.is_dry_run,
            reason: "fixture".to_owned(),
        }])
    }
}

async fn sweeping_manager(
    config: Arc<Config>,
    records: &[WarmImageRecord],
) -> (AllocationManager, Arc<SweepingWorker>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = hot_store(directory.path()).await;
    for record in records {
        images
            .save(record)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    let worker = Arc::new(SweepingWorker::default());
    (
        AllocationManager::new(
            ConfigHandle::new(config),
            store,
            images,
            hot,
            worker.clone(),
        ),
        worker,
        directory,
    )
}

/// A dry run that showed nothing and then retired generations on a real run
/// would be a dry-run asymmetry against the name-addressed backend.
#[tokio::test]
async fn a_dry_run_lists_retirement_candidates_and_deletes_nothing() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let (manager, worker, _directory) =
        sweeping_manager(config, &[record("project-warm", 3)]).await;

    let report = manager
        .sweep(true)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(report.retired_images.len(), 1);
    assert!(!report.retired_images[0].is_deleted);
    assert!(report.deleted.is_empty());
    assert!(report.pruned_records.is_empty());
    assert!(worker.seen()[0].is_dry_run);
}

/// The sweep runs after the prune in the same pass, so a pointer whose profile
/// stopped declaring `warm_template` is collected in the pass that dropped its
/// record — and a dry run sees the same candidate without applying the prune.
#[tokio::test]
async fn image_retirement_sees_the_prune_set_on_a_dry_run_and_a_real_run() {
    let config = config_without_the_recorded_profile("/tmp/flanforge-sweep".into());
    let (manager, worker, _directory) =
        sweeping_manager(Arc::clone(&config), &[record("project-retired", 2)]).await;

    let dry = manager
        .sweep(true)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert!(dry.pruned_records.is_empty(), "a dry run prunes nothing");
    let seen = worker.seen();
    assert!(
        !seen[0].claimed.contains(&test_support::profile_name()),
        "the dry run must see the prune set it would have applied"
    );
    assert!(!seen[0].declared.contains_key(&test_support::profile_name()));

    let real = manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    assert_eq!(real.pruned_records, vec![test_support::profile_name()]);
    assert!(real.retired_images[0].is_deleted);
    let seen = worker.seen();
    assert!(!seen[1].claimed.contains(&test_support::profile_name()));
}

/// A profile that still declares its warm template keeps its record, and the
/// backend is told so.
#[tokio::test]
async fn a_declared_profile_is_reported_as_claimed_to_the_backend() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let (manager, worker, _directory) =
        sweeping_manager(config, &[record("project-warm", 3)]).await;
    manager
        .sweep(false)
        .await
        .unwrap_or_else(|error| unreachable!("sweep: {error}"));
    let seen = worker.seen();
    assert!(seen[0].claimed.contains(&test_support::profile_name()));
    assert_eq!(
        seen[0].declared.get(&test_support::profile_name()),
        Some(&VmName::new("project-warm").unwrap_or_else(|error| unreachable!("{error}")))
    );
}

/// `claimed` is an authority rather than a hint: an empty set tells the backend
/// that no profile claims anything, which promotes a live generation to a
/// deletion candidate. A pass that cannot read the records must therefore never
/// reach retirement at all.
#[tokio::test]
async fn an_unreadable_record_authority_never_offers_a_retirement_candidate() {
    let config = test_support::warm_config("/tmp/flanforge-sweep".into());
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(crate::tests::UnavailableWarmImageStore);
    let hot = hot_store(directory.path()).await;
    let worker = Arc::new(SweepingWorker::default());
    let manager = AllocationManager::new(
        ConfigHandle::new(config),
        store,
        images,
        hot,
        worker.clone(),
    );

    assert!(manager.sweep(false).await.is_err());
    assert!(
        worker.seen().is_empty(),
        "an unreadable authority may not be read as an empty one"
    );
}

fn hot_guest(name: &str, state: HotState) -> HotGuest {
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
    guest.state = state;
    guest
}

/// A pool machine belongs to its record, not to an allocation, so the prefix
/// authorization must not collect it however old it is.
#[test]
fn a_hot_machine_is_protected_by_its_record_until_the_record_is_terminal() {
    let config = test_support::hot_config("/tmp/flanforge-sweep".into());
    let name = "ci-hot-project-0123456789ab";
    let machines = [machine(name, DAY)];
    let unaged = [HostMachine {
        age_seconds: None,
        ..machine(name, DAY)
    }];

    for state in [HotState::Provisioning, HotState::Idle, HotState::Draining] {
        let hot = [hot_guest(name, state)];
        let inputs = ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &[],
            images: &[],
            hot: &hot,
        };
        assert!(plan_sweep(inputs).is_empty(), "{state:?}");
        assert!(
            unaged_candidates(ReapInputs {
                machines: &unaged,
                ..inputs
            })
            .is_empty(),
            "{state:?} was reported as an uncollectable orphan"
        );
    }

    let evicted = [hot_guest(name, HotState::Evicted)];
    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[],
        hot: &evicted,
    });
    assert_eq!(
        planned
            .iter()
            .map(|candidate| candidate.authorization.clone())
            .collect::<Vec<_>>(),
        vec![ReapAuthorization::Prefix],
        "an evicted record no longer names a machine"
    );
}
