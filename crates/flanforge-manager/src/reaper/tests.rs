use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationMode, AllocationState, BaseFingerprint, Config, Profile, RunnerLabel,
    VmName, WarmImageRecord, WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    super::{
        AllocationManager, AllocationReporter, AllocationWorker, ConfigHandle, HostMachine,
        MachineState, ReapRequest, WorkerError,
    },
    ReapAuthorization, ReapInputs, plan_sweep,
};

#[derive(Debug)]
struct HostWorker {
    machines: Vec<HostMachine>,
    deleted: Mutex<Vec<String>>,
    /// Applied on the first deletion, so a test can reload configuration in the
    /// middle of a sweep exactly as the watcher would.
    reload: Mutex<Option<(Arc<ConfigHandle>, Arc<Config>)>>,
}

impl HostWorker {
    fn new(machines: Vec<HostMachine>) -> Self {
        Self {
            machines,
            deleted: Mutex::new(Vec::new()),
            reload: Mutex::new(None),
        }
    }

    fn reloading(machines: Vec<HostMachine>, handle: Arc<ConfigHandle>, next: Arc<Config>) -> Self {
        Self {
            machines,
            deleted: Mutex::new(Vec::new()),
            reload: Mutex::new(Some((handle, next))),
        }
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted
            .lock()
            .map(|deleted| deleted.clone())
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

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(self.machines.clone())
    }

    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        if request.authorization == &ReapAuthorization::Prefix {
            return Err(WorkerError::new("a prefix candidate is never deletable"));
        }
        if request.reserved.contains(request.name.as_str()) {
            return Err(WorkerError::new("live configuration claims that name"));
        }
        if let Ok(mut deleted) = self.deleted.lock() {
            deleted.push(request.name.to_string());
        }
        if let Ok(mut reload) = self.reload.lock()
            && let Some((handle, next)) = reload.take()
        {
            let _ = handle.apply(next);
        }
        Ok(())
    }
}

fn machine(name: &str, age_seconds: u64) -> HostMachine {
    HostMachine {
        name: name.to_owned(),
        state: MachineState::Stopped,
        age_seconds: Some(age_seconds),
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
    });
    assert!(planned.is_empty(), "{planned:?}");
}

#[test]
fn plan_sweep_reports_a_prefix_only_name_and_never_deletes_it() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [machine("ci-unknown-9-1", DAY)];

    let planned = plan_sweep(ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[],
    });
    assert_eq!(planned.len(), 1);
    assert_eq!(
        planned.first().map(|candidate| &candidate.authorization),
        Some(&ReapAuthorization::Prefix)
    );
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
    });
    assert_eq!(planned.len(), 1);
    assert_eq!(
        planned.first().map(|candidate| &candidate.authorization),
        Some(&ReapAuthorization::Image(test_support::profile_name()))
    );
}

#[test]
fn a_candidate_below_the_age_floor_or_without_an_age_is_skipped() {
    let config = test_support::config("/tmp/flanforge-sweep".into());
    let machines = [
        machine("ci-project-41-1", 60),
        HostMachine {
            name: "ci-project-40-1".to_owned(),
            state: MachineState::Stopped,
            age_seconds: None,
        },
    ];

    assert!(
        plan_sweep(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &[],
            images: &[],
        })
        .is_empty()
    );
}

async fn manager(
    config: Arc<Config>,
    machines: Vec<HostMachine>,
) -> (AllocationManager, Arc<HostWorker>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let worker = Arc::new(HostWorker::new(machines));
    (
        AllocationManager::new(ConfigHandle::new(config), store, images, worker.clone()),
        worker,
        directory,
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

#[tokio::test]
async fn a_sweep_deletes_a_recorded_orphan_and_leaves_a_prefix_only_name() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
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
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let worker = Arc::new(HostWorker::new(vec![
        machine("ci-project-41-1", DAY),
        machine("ci-stranger-1-1", DAY),
    ]));
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images,
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
    assert_eq!(report.deleted, vec!["ci-project-41-1".to_owned()]);
    assert_eq!(report.skipped, vec!["ci-stranger-1-1".to_owned()]);
    assert_eq!(worker.deleted(), vec!["ci-project-41-1".to_owned()]);
}

/// A manager whose warm image store already holds `records`.
async fn manager_with_records(
    config: Arc<Config>,
    worker: Arc<HostWorker>,
    records: &[WarmImageRecord],
) -> (AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    for record in records {
        images
            .save(record)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    (
        AllocationManager::new(ConfigHandle::new(config), store, images, worker),
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
    claimed.runtime.tart_home = Some("/opt/flanforge/tart".into());
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
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    for stored in [record("project-retired", 2), claimed_record()] {
        images
            .save(&stored)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    let manager = AllocationManager::new(handle, store, images, worker.clone());

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
