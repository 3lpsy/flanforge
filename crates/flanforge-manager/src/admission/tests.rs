use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationState, Config, GuestSize, Profile,
    ProfileName, RequestOptions, RunnerLabel, VmName,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore};
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, BusyReason, ConfigHandle, HostMachine,
    MachineState, ManagerError, WorkerError, service::Entry,
};

/// A worker that never lists the host, so admission answers busy.
#[derive(Debug)]
struct BlindWorker;

#[async_trait]
impl AllocationWorker for BlindWorker {
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
        Err(WorkerError::new("cannot list the host"))
    }
}

fn budgeted(state_dir: std::path::PathBuf, cpu_count: u8, memory_mb: u32) -> Arc<Config> {
    let mut config = (*test_support::config(state_dir)).clone();
    config.runtime.host_cpu_count = Some(cpu_count);
    config.runtime.host_memory_mb = Some(memory_mb);
    Arc::new(config)
}

fn allocation(name: &str, size: Option<GuestSize>) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    allocation.state = AllocationState::Ready;
    allocation.size = size;
    allocation
}

fn entries(allocations: Vec<Allocation>) -> HashMap<AllocationId, Entry> {
    allocations
        .into_iter()
        .map(|allocation| {
            let (sender, _) = watch::channel(allocation.clone());
            (
                allocation.id,
                Entry {
                    allocation,
                    sender,
                    cancellation: CancellationToken::new(),
                    is_supervised: true,
                    is_terminalizing: false,
                },
            )
        })
        .collect()
}

fn running(name: &str) -> HostMachine {
    HostMachine {
        name: name.to_owned(),
        state: MachineState::Running,
        age_seconds: Some(10),
    }
}

fn busy_reason(result: &Result<(), ManagerError>) -> Option<BusyReason> {
    match result {
        Err(ManagerError::Busy { reason, .. }) => Some(*reason),
        _ => None,
    }
}

#[test]
fn two_fitting_requests_are_admitted_and_the_third_is_busy() {
    let config = budgeted("/private/state".into(), 8, 16_384);
    let size = test_support::size();
    assert!(
        AllocationManager::ensure_capacity(&entries(Vec::new()), &config, size, &[]).is_ok(),
        "an empty host admits the first request"
    );
    let one = entries(vec![allocation("ci-one", Some(size))]);
    assert!(AllocationManager::ensure_capacity(&one, &config, size, &[]).is_ok());
    let two = entries(vec![
        allocation("ci-one", Some(size)),
        allocation("ci-two", Some(size)),
    ]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &two,
            &config,
            size,
            &[]
        )),
        Some(BusyReason::Slots)
    );
}

#[test]
fn the_budget_binds_in_both_dimensions() {
    let size = test_support::size();
    let held = entries(vec![allocation("ci-one", Some(size))]);
    let by_cpu = budgeted("/private/state".into(), 6, 65_536);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &by_cpu,
            size,
            &[]
        )),
        Some(BusyReason::Budget)
    );
    let by_memory = budgeted("/private/state".into(), 64, 12_288);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &by_memory,
            size,
            &[]
        )),
        Some(BusyReason::Budget)
    );
}

#[test]
fn a_foreign_running_vm_consumes_a_slot() {
    let config = budgeted("/private/state".into(), 64, 65_536);
    let size = test_support::size();
    let held = entries(vec![allocation("ci-one", Some(size))]);
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[]).is_ok());
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[running("devboxvm")]
        )),
        Some(BusyReason::Slots)
    );
    // Our own running clone is not counted twice.
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[running("ci-one")]).is_ok());
}

#[test]
fn an_unset_budget_serializes_exactly_as_before() {
    let config = test_support::config("/private/state".into());
    let size = test_support::size();
    assert!(AllocationManager::ensure_capacity(&entries(Vec::new()), &config, size, &[]).is_ok());
    let held = entries(vec![allocation("ci-one", Some(size))]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[]
        )),
        Some(BusyReason::Serialized)
    );
}

#[test]
fn a_pre_change_record_charges_its_profile_values() {
    let config = budgeted("/private/state".into(), 6, 65_536);
    let size = test_support::size();
    let held = entries(vec![allocation("ci-one", None)]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[]
        )),
        Some(BusyReason::Budget)
    );

    // A record whose own profile is gone still charges the largest profile
    // configured: a guest of unknown size is not a guest that costs nothing.
    let mut orphaned = (*config).clone();
    let name = ProfileName::new("other").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    orphaned.profiles = std::collections::BTreeMap::from([(name, test_support::profile())]);
    orphaned.runtime.host_cpu_count = Some(6);
    let orphaned = Arc::new(orphaned);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &orphaned,
            size,
            &[]
        )),
        Some(BusyReason::Budget)
    );
}

/// RUN-562: a guest still running under a terminal record is capacity the host
/// has already spent, so the budget charges it as well as the slot.
#[test]
fn a_running_orphan_is_charged_against_the_budget() {
    let size = test_support::size();
    let mut orphan = allocation("ci-orphan", Some(size));
    orphan.state = AllocationState::Failed;
    orphan.vm_created = true;
    let held = entries(vec![orphan]);
    let machines = [running("ci-orphan")];

    let tight = budgeted("/private/state".into(), 6, 65_536);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held, &tight, size, &machines
        )),
        Some(BusyReason::Budget)
    );

    let roomy = budgeted("/private/state".into(), 8, 65_536);
    assert!(AllocationManager::ensure_capacity(&held, &roomy, size, &machines).is_ok());

    // A machine no record claims stays slot-only, as the design intends.
    let unclaimed = [running("someones-laptop")];
    assert!(AllocationManager::ensure_capacity(&HashMap::new(), &tight, size, &unclaimed).is_ok());
}

/// RUN-524: one profile's three image names are owned by one producer.
#[test]
fn a_second_regeneration_of_one_profile_is_refused() {
    let mut producing = allocation("ci-one", Some(test_support::size()));
    producing.mode = AllocationMode::Regenerate;
    let held = entries(vec![producing]);

    assert_eq!(
        busy_reason(&AllocationManager::ensure_sole_producer(
            &held,
            &test_support::profile_name(),
            AllocationMode::Regenerate,
        )),
        Some(BusyReason::Regenerating)
    );
    // The exclusion is scoped to production and to the profile.
    for consuming in [AllocationMode::Warm, AllocationMode::Cold] {
        assert!(
            AllocationManager::ensure_sole_producer(
                &held,
                &test_support::profile_name(),
                consuming,
            )
            .is_ok()
        );
    }
    let other = ProfileName::new("other").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(
        AllocationManager::ensure_sole_producer(&held, &other, AllocationMode::Regenerate).is_ok()
    );
}

#[test]
fn a_terminal_allocation_releases_its_capacity() {
    let config = budgeted("/private/state".into(), 6, 65_536);
    let size = test_support::size();
    let mut terminal = allocation("ci-one", Some(size));
    terminal.state = AllocationState::Completed;
    let held = entries(vec![terminal]);
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[]).is_ok());
}

#[tokio::test]
async fn a_failed_listing_answers_busy() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(test_support::config(directory.path().to_path_buf())),
        store,
        images,
        Arc::new(BlindWorker),
    );
    let result = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await;
    assert!(matches!(
        result,
        Err(ManagerError::Busy {
            reason: BusyReason::ProbeFailed,
            holder: None
        })
    ));
}
