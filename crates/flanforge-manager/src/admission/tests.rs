use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationOrigin, AllocationState, CloneKind,
    CloneSource, Config, GuestSize, HotGuest, HotLane, HotState, Profile, ProfileName,
    RequestOptions, RunnerLabel, VmName,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use flanforge_store::AllocationStore;
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, BusyReason, CleanupBudget,
    ConfigHandle, HostMachine, MachineOwnership, MachineState, ManagerError, WorkerError,
    service::Entry,
};
use super::{committed_capacity, storage::floored};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Err(WorkerError::new("cannot list the host"))
    }
}

/// A worker whose boot source is larger than the profile that names it.
#[derive(Debug)]
struct OversizedBase(u64);

#[async_trait]
impl AllocationWorker for OversizedBase {
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

    async fn base_storage_mb(&self, _profile: &ProfileName, _source: &CloneSource) -> Option<u64> {
        Some(self.0)
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
        size: None,
        ownership: MachineOwnership::Unknown,
    }
}

fn running_with(name: &str, ownership: MachineOwnership) -> HostMachine {
    HostMachine {
        ownership,
        ..running(name)
    }
}

/// An idle pool machine, sized by its own record.
fn hot_guest(name: &str) -> HotGuest {
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
    guest
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
        AllocationManager::ensure_capacity(&entries(Vec::new()), &config, size, &[], &[]).is_ok(),
        "an empty host admits the first request"
    );
    let one = entries(vec![allocation("ci-one", Some(size))]);
    assert!(AllocationManager::ensure_capacity(&one, &config, size, &[], &[]).is_ok());
    let two = entries(vec![
        allocation("ci-one", Some(size)),
        allocation("ci-two", Some(size)),
    ]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &two,
            &config,
            size,
            &[],
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
            &[],
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
            &[],
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
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[], &[]).is_ok());
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[running("devboxvm")],
            &[]
        )),
        Some(BusyReason::Slots)
    );
    // Our own running clone is not counted twice.
    assert!(
        AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[running_with("ci-one", MachineOwnership::Owned)],
            &[]
        )
        .is_ok()
    );
}

/// RUN-727: a colliding name is allocation evidence only when metadata proves
/// that the machine belongs to this service instance.
#[test]
fn active_name_collisions_are_classified_by_machine_ownership() {
    let config = budgeted("/private/state".into(), 64, 65_536);
    let size = test_support::size();
    let held = entries(vec![allocation("ci-one", Some(size))]);
    for ownership in [MachineOwnership::Foreign, MachineOwnership::Unknown] {
        assert_eq!(
            busy_reason(&AllocationManager::ensure_capacity(
                &held,
                &config,
                size,
                &[running_with("ci-one", ownership)],
                &[]
            )),
            Some(BusyReason::Slots),
            "{ownership:?} collision was treated as owned"
        );
    }
    assert!(
        AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[running_with("ci-one", MachineOwnership::Owned)],
            &[]
        )
        .is_ok()
    );
}

/// RUN-727: a terminal record charges only a metadata-owned survivor. Foreign
/// and unknown collisions remain foreign and unknown size fails closed.
#[test]
fn terminal_name_collisions_are_classified_by_machine_ownership() {
    let size = test_support::size();
    let mut terminal = allocation("ci-one", Some(size));
    terminal.state = AllocationState::Completed;
    terminal.vm_created = true;
    let held = entries(vec![terminal]);
    let config = budgeted("/private/state".into(), 64, 65_536);

    let mut owned = running_with("ci-one", MachineOwnership::Owned);
    owned.size = None;
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[owned], &[]).is_ok());

    let mut foreign = running_with("ci-one", MachineOwnership::Foreign);
    foreign.size = Some(size);
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[foreign], &[]).is_ok());

    for ownership in [MachineOwnership::Foreign, MachineOwnership::Unknown] {
        let unknown = running_with("ci-one", ownership);
        assert_eq!(
            busy_reason(&AllocationManager::ensure_capacity(
                &held,
                &config,
                size,
                &[unknown],
                &[]
            )),
            Some(BusyReason::Budget),
            "{ownership:?} unknown-size collision did not fail closed"
        );
    }
}

#[test]
fn an_unset_budget_serializes_exactly_as_before() {
    let config = test_support::config("/private/state".into());
    let size = test_support::size();
    assert!(
        AllocationManager::ensure_capacity(&entries(Vec::new()), &config, size, &[], &[]).is_ok()
    );
    let held = entries(vec![allocation("ci-one", Some(size))]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[],
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
            &[],
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
            &[],
            &[]
        )),
        Some(BusyReason::Budget)
    );
}

/// RUN-613: an unknown-size guest is charged the independent maximum of every
/// resource, not the lexicographic maximum of one profile tuple.
#[test]
fn an_unknown_guest_uses_component_wise_profile_maxima() {
    let mut config = (*test_support::config("/private/state".into())).clone();
    let mut cpu_heavy = test_support::profile();
    cpu_heavy.cpu_count = 16;
    cpu_heavy.memory_mb = 4_096;
    cpu_heavy.storage_mb = 20_000;
    let mut storage_heavy = test_support::profile();
    storage_heavy.cpu_count = 2;
    storage_heavy.memory_mb = 32_768;
    storage_heavy.storage_mb = 100_000;
    config.profiles = std::collections::BTreeMap::from([
        (
            ProfileName::new("cpu").unwrap_or_else(|error| unreachable!("fixture: {error}")),
            cpu_heavy,
        ),
        (
            ProfileName::new("storage").unwrap_or_else(|error| unreachable!("fixture: {error}")),
            storage_heavy,
        ),
    ]);
    let unknown = allocation("ci-unknown", None);

    assert_eq!(
        committed_capacity(&[&unknown], &config),
        super::capacity::CommittedCapacity {
            active: 1,
            cpu_count: 16,
            memory_mb: 32_768,
            storage_mb: 100_000,
        }
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
    let machines = [running_with("ci-orphan", MachineOwnership::Owned)];

    let tight = budgeted("/private/state".into(), 6, 65_536);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &tight,
            size,
            &machines,
            &[]
        )),
        Some(BusyReason::Budget)
    );

    let roomy = budgeted("/private/state".into(), 8, 65_536);
    assert!(AllocationManager::ensure_capacity(&held, &roomy, size, &machines, &[]).is_ok());

    // A machine no record claims with unknown size fails closed under a budget.
    let unclaimed = [running("someones-laptop")];
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &HashMap::new(),
            &tight,
            size,
            &unclaimed,
            &[]
        )),
        Some(BusyReason::Budget)
    );
}

#[test]
fn a_known_foreign_guest_is_charged_in_every_dimension() {
    let mut config = (*budgeted("/private/state".into(), 8, 16_384)).clone();
    config.runtime.max_running_vms = 2;
    config.runtime.host_storage_mb = Some(100_000);
    let mut machine = running("developer-vm");
    machine.size = Some(GuestSize {
        cpu_count: 4,
        memory_mb: 8_192,
        storage_mb: 60_000,
    });
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &HashMap::new(),
            &config,
            test_support::size(),
            &[machine],
            &[]
        )),
        Some(BusyReason::Budget)
    );
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
    assert!(AllocationManager::ensure_capacity(&held, &config, size, &[], &[]).is_ok());
}

#[tokio::test]
async fn a_failed_listing_answers_busy() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let manager = manager_for(&directory, config, Arc::new(BlindWorker)).await;
    let result = created(&manager).await;
    assert!(matches!(
        result,
        Err(ManagerError::Busy {
            reason: BusyReason::ProbeFailed,
            holder: None
        })
    ));
}

/// The guest's disk is `max(storage_mb, base virtual size)`, so a base larger
/// than the profile that names it is what the budget has to reserve.
#[test]
fn a_base_larger_than_the_profile_raises_the_charged_storage() {
    let size = test_support::size();
    for (base_mb, expected, reason) in [
        (20_480, size.storage_mb, "a base smaller than the profile"),
        (size.storage_mb, size.storage_mb, "an exactly-sized base"),
        (81_920, 81_920, "a base larger than the profile"),
        (
            u64::MAX,
            GuestSize::MAX_STORAGE_MB,
            "a raise clamped to what a record can carry",
        ),
    ] {
        assert_eq!(floored(size, base_mb).storage_mb, expected, "{reason}");
    }
    // Only storage moves; the request's own choices are untouched.
    assert_eq!(floored(size, 81_920).cpu_count, size.cpu_count);
    assert_eq!(floored(size, 81_920).memory_mb, size.memory_mb);
}

/// The record carries the disk the guest actually gets, so every later charge
/// — this allocation's and the next one's — reads the effective size.
#[tokio::test]
async fn a_larger_base_is_recorded_rather_than_the_declared_size() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*budgeted(directory.path().to_path_buf(), 64, 65_536)).clone();
    config.runtime.host_storage_mb = Some(131_072);
    let manager = manager_for(
        &directory,
        Arc::new(config),
        Arc::new(OversizedBase(81_920)),
    )
    .await;
    let allocation = created(&manager)
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(
        allocation.size.map(|size| size.storage_mb),
        Some(81_920),
        "the declared 40 GiB is not what this guest gets"
    );
}

/// The over-commit half of the defect: a base the budget could never have
/// fitted is refused, rather than admitted against the declared size.
#[tokio::test]
async fn a_base_that_does_not_fit_the_budget_answers_busy() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*budgeted(directory.path().to_path_buf(), 64, 65_536)).clone();
    config.runtime.host_storage_mb = Some(65_536);
    let manager = manager_for(
        &directory,
        Arc::new(config),
        Arc::new(OversizedBase(81_920)),
    )
    .await;
    assert_eq!(
        busy_reason(&created(&manager).await.map(|_| ())),
        Some(BusyReason::Budget),
        "the declared 40 GiB fits this budget and the 80 GiB base does not"
    );
}

async fn created(manager: &AllocationManager) -> Result<Allocation, ManagerError> {
    manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .map(|created| created.allocation)
}

async fn manager_for(
    directory: &tempfile::TempDir,
    config: Arc<Config>,
    worker: Arc<dyn AllocationWorker>,
) -> AllocationManager {
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    AllocationManager::new(ConfigHandle::new(config), store, images, hot, worker)
}

/// The fail-closed bug this phase exists to prevent: an idle hot guest is
/// running and owned by no active allocation, and Tart reports no size, so
/// `foreign_capacity` marked the host unknown-sized and refused every
/// subsequent allocation for good. The record is what classifies it.
#[test]
fn an_idle_hot_machine_is_not_a_foreign_workload_of_unknown_size() {
    let config = budgeted("/private/state".into(), 64, 65_536);
    let size = test_support::size();
    let pooled = running_with("ci-hot-project-0123456789ab", MachineOwnership::Owned);
    assert!(pooled.size.is_none(), "the fixture must be unsized");
    assert!(config.runtime.host_cpu_count.is_some() && config.runtime.host_memory_mb.is_some());

    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &entries(Vec::new()),
            &config,
            size,
            std::slice::from_ref(&pooled),
            &[],
        )),
        Some(BusyReason::Budget),
        "an unsized machine no record claims is still charged fail-closed"
    );
    assert!(
        AllocationManager::ensure_capacity(
            &entries(Vec::new()),
            &config,
            size,
            &[pooled],
            &[hot_guest("ci-hot-project-0123456789ab")],
        )
        .is_ok()
    );
}

/// The honest cost: the machine holds a slot and its recorded size whether or
/// not anyone is on it, so on a two-slot host it halves concurrency.
#[test]
fn an_idle_hot_machine_holds_one_slot_and_its_recorded_size() {
    let config = budgeted("/private/state".into(), 64, 65_536);
    assert_eq!(config.runtime.max_running_vms, 2);
    let size = test_support::size();
    let pooled = running_with("ci-hot-project-0123456789ab", MachineOwnership::Owned);
    let hot = [hot_guest("ci-hot-project-0123456789ab")];

    let held = entries(vec![allocation("ci-one", Some(size))]);
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &held,
            &config,
            size,
            &[
                running_with("ci-one", MachineOwnership::Owned),
                pooled.clone()
            ],
            &hot,
        )),
        Some(BusyReason::Slots),
        "the pool machine takes the second of two slots"
    );

    // Four CPUs held by the pool plus four requested exceeds a four-CPU host.
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &entries(Vec::new()),
            &budgeted("/private/state".into(), 4, 16_384),
            size,
            std::slice::from_ref(&pooled),
            &hot,
        )),
        Some(BusyReason::Budget)
    );
}

/// A claimed machine is charged through the allocation holding it. Charging it
/// twice would refuse a host that is exactly full rather than merely full.
#[test]
fn a_claimed_hot_machine_is_charged_once_through_its_allocation() {
    let size = test_support::size();
    let name = "ci-hot-project-0123456789ab";
    let held = entries(vec![allocation(name, Some(size))]);
    let mut claimed = hot_guest(name);
    claimed
        .ensure_claimed(AllocationId::new())
        .unwrap_or_else(|error| unreachable!("claim: {error}"));

    assert!(
        AllocationManager::ensure_capacity(
            &held,
            &budgeted("/private/state".into(), 8, 16_384),
            size,
            &[running_with(name, MachineOwnership::Owned)],
            &[claimed],
        )
        .is_ok()
    );
}

/// RUN-727's rule applied to the pool: a record naming a machine this service
/// cannot prove it owns classifies nothing, so that machine stays on the
/// fail-closed foreign path instead of going uncharged.
#[test]
fn a_hot_record_over_an_unowned_machine_classifies_nothing() {
    let config = budgeted("/private/state".into(), 64, 65_536);
    let name = "ci-hot-project-0123456789ab";
    let hot = [hot_guest(name)];
    for ownership in [MachineOwnership::Foreign, MachineOwnership::Unknown] {
        assert_eq!(
            busy_reason(&AllocationManager::ensure_capacity(
                &entries(Vec::new()),
                &config,
                test_support::size(),
                &[running_with(name, ownership)],
                &hot,
            )),
            Some(BusyReason::Budget),
            "{ownership:?} was classified as a pool machine"
        );
    }
}

/// The two arms `hot_names` cannot answer on its own. A record is written the
/// moment a finished allocation's guest is handed over, which is before the
/// next host listing has to agree, so `Provisioning` is charged from the record
/// alone; and a machine an allocation reuses is charged through that
/// allocation, so charging it here too would double-count the host.
#[test]
fn a_handover_is_charged_from_its_record_and_a_reused_machine_only_once() {
    let size = test_support::size();
    let name = "ci-hot-project-0123456789ab";

    // Nothing on the host listing yet, and the slot is already the pool's.
    let mut handover = hot_guest(name);
    handover.state = HotState::Provisioning;
    assert_eq!(
        busy_reason(&AllocationManager::ensure_capacity(
            &entries(Vec::new()),
            &budgeted("/private/state".into(), 4, 8_192),
            size,
            &[],
            std::slice::from_ref(&handover),
        )),
        Some(BusyReason::Budget),
        "a machine mid-handover went uncharged"
    );

    // The reusing allocation's own `vm_name` is a fresh name that never becomes
    // a machine; the machine it holds is the one its origin names.
    let mut reuser = allocation("ci-project-1-1", Some(size));
    reuser.set_hot(
        Some(HotLane::Protected),
        AllocationOrigin::HotReuse {
            vm_name: VmName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
            jobs_served: 1,
            booted_at_unix: 0,
        },
        None,
        None,
    );
    let mut claimed = hot_guest(name);
    claimed
        .ensure_claimed(reuser.id)
        .unwrap_or_else(|error| unreachable!("claim: {error}"));
    assert!(
        AllocationManager::ensure_capacity(
            &entries(vec![reuser]),
            &budgeted("/private/state".into(), 8, 16_384),
            size,
            &[running_with(name, MachineOwnership::Owned)],
            &[claimed],
        )
        .is_ok(),
        "the reused machine was charged twice"
    );
}
