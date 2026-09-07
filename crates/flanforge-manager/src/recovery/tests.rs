use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, BaseFingerprint, CloneKind, CloneSource, Config, HotGuest, HotLane, HotState,
    Profile, VmName, WarmGeneration, WarmImageRecord, WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, HotGuestStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
    HostMachine, MachineOwnership, MachineState, ReapRequest, WorkerError,
};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

/// Records the image operations reconciliation performs, in order.
#[derive(Debug)]
struct ImageWorker {
    machines: Vec<HostMachine>,
    actions: Mutex<Vec<String>>,
    is_inventory_available: bool,
    /// Whether the recycle gate passes. Adoption needs proof, so most cases
    /// want it refusing; the drained-record case needs it passing to show the
    /// record, not the gate, is what refuses.
    is_gate_passing: bool,
}

impl ImageWorker {
    fn new(present: &[&str]) -> Self {
        Self {
            machines: present
                .iter()
                .map(|name| HostMachine {
                    name: (*name).to_owned(),
                    state: MachineState::Stopped,
                    age_seconds: Some(10),
                    size: None,
                    ownership: MachineOwnership::Unknown,
                })
                .collect(),
            actions: Mutex::new(Vec::new()),
            is_inventory_available: true,
            is_gate_passing: false,
        }
    }

    fn gating(present: &[&str]) -> Self {
        Self {
            is_gate_passing: true,
            ..Self::new(present)
        }
    }

    fn unavailable() -> Self {
        Self {
            machines: Vec::new(),
            actions: Mutex::new(Vec::new()),
            is_inventory_available: false,
            is_gate_passing: false,
        }
    }

    fn record(&self, action: String) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push(action);
        }
    }

    fn actions(&self) -> Vec<String> {
        self.actions
            .lock()
            .map(|actions| actions.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl AllocationWorker for ImageWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn ensure_hot_reset(
        &self,
        guest: &flanforge_core::HotGuest,
        _reset: super::super::HotReset,
    ) -> Result<(), WorkerError> {
        self.record(format!("hot-reset {}", guest.vm_name));
        if self.is_gate_passing {
            return Ok(());
        }
        Err(WorkerError::new("the recycle gate is not implemented here"))
    }

    async fn ensure_hot_evicted(
        &self,
        guest: &flanforge_core::HotGuest,
        _budget: super::super::CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.record(format!("hot-evict {}", guest.vm_name));
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
        if self.is_inventory_available {
            Ok(self.machines.clone())
        } else {
            Err(WorkerError::new("host inventory unavailable"))
        }
    }

    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        self.record(format!("delete {}", request.name));
        Ok(())
    }

    /// The name-addressed restore: clone `<warm>.previous` back over the live
    /// name, refusing when nothing survives. `tart clone` refuses an existing
    /// destination, so the fake does too.
    async fn ensure_warm_restored(
        &self,
        _profile: &Profile,
        record: &WarmImageRecord,
    ) -> Result<(), WorkerError> {
        let previous = record
            .previous_name()
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let destination = &record.warm_template;
        if !self
            .machines
            .iter()
            .any(|machine| machine.name == previous.as_str())
        {
            return Err(WorkerError::new(
                "no warm image survives this profile; allocations boot cold",
            ));
        }
        self.record(format!("clone {previous} {destination}"));
        if self
            .machines
            .iter()
            .any(|machine| machine.name == destination.as_str())
        {
            return Err(WorkerError::new("destination image already exists"));
        }
        Ok(())
    }
}

fn fingerprint(value: &str) -> BaseFingerprint {
    BaseFingerprint::new(value).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn record(state: WarmImageState, has_previous: bool) -> WarmImageRecord {
    WarmImageRecord {
        profile: test_support::profile_name(),
        warm_template: VmName::new("project-warm")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        generation: 5,
        base_fingerprint: fingerprint("aa01"),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1_755_324_251,
        state,
        previous: has_previous.then(|| WarmGeneration {
            generation: 4,
            base_fingerprint: fingerprint("9900"),
            produced_by: flanforge_core::AllocationId::new(),
            produced_at_unix: 1_755_000_000,
        }),
    }
}

struct Reconciled {
    actions: Vec<String>,
    record: Option<WarmImageRecord>,
}

async fn reconcile(stored: WarmImageRecord, present: &[&str]) -> Reconciled {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config: Arc<Config> = test_support::warm_config(directory.path().to_path_buf());
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
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    images
        .save(&stored)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let worker = Arc::new(ImageWorker::new(present));
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images.clone(),
        hot,
        worker.clone(),
    );
    manager
        .ensure_warm_consistent(&config)
        .await
        .unwrap_or_else(|error| unreachable!("reconcile: {error}"));
    Reconciled {
        actions: worker.actions(),
        record: images
            .load(&test_support::profile_name())
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
    }
}

#[tokio::test]
async fn each_two_phase_record_state_resolves_as_tabulated() {
    // Promoted, image present, candidate present: the candidate is removed and
    // the record is untouched.
    let promoted = reconcile(
        record(WarmImageState::Promoted, true),
        &[
            "project-warm",
            "project-warm.staging",
            "project-warm.previous",
        ],
    )
    .await;
    assert_eq!(promoted.actions, vec!["delete project-warm.staging"]);
    assert_eq!(
        promoted
            .record
            .map(|record| (record.state, record.generation)),
        Some((WarmImageState::Promoted, 5))
    );

    // Promoted and consistent: nothing happens at all.
    let quiet = reconcile(record(WarmImageState::Promoted, true), &["project-warm"]).await;
    assert!(quiet.actions.is_empty(), "{:?}", quiet.actions);

    // Staging with the candidate gone: step 6 finished but was never recorded.
    let finalized = reconcile(record(WarmImageState::Staging, true), &["project-warm"]).await;
    assert!(finalized.actions.is_empty(), "{:?}", finalized.actions);
    assert_eq!(
        finalized
            .record
            .map(|record| (record.state, record.generation)),
        Some((WarmImageState::Promoted, 5))
    );

    // Staging with the candidate still present: retirement never ran, so the
    // live image is already the surviving generation and only the record moves.
    let interrupted = reconcile(
        record(WarmImageState::Staging, true),
        &[
            "project-warm",
            "project-warm.staging",
            "project-warm.previous",
        ],
    )
    .await;
    assert_eq!(interrupted.actions, vec!["delete project-warm.staging"]);
    assert_eq!(
        interrupted
            .record
            .map(|record| (record.state, record.generation, record.base_fingerprint)),
        Some((WarmImageState::Promoted, 4, fingerprint("9900")))
    );

    // The image is gone and a rollback generation survives.
    let restored = reconcile(
        record(WarmImageState::Promoted, true),
        &["project-warm.previous"],
    )
    .await;
    assert_eq!(
        restored.actions,
        vec!["clone project-warm.previous project-warm"]
    );

    // Nothing survives: the record is dropped so the next job boots cold.
    let lost = reconcile(record(WarmImageState::Promoted, false), &[]).await;
    assert!(lost.actions.is_empty(), "{:?}", lost.actions);
    assert_eq!(lost.record, None);
}

#[tokio::test]
async fn reconciliation_never_promotes_a_staged_candidate() {
    let staged = reconcile(
        record(WarmImageState::Staging, false),
        &["project-warm.staging"],
    )
    .await;
    assert_eq!(staged.actions, vec!["delete project-warm.staging"]);
    assert_eq!(staged.record, None);
}

#[tokio::test]
async fn unavailable_inventory_aborts_reconciliation_without_mutation() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config: Arc<Config> = test_support::warm_config(directory.path().to_path_buf());
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
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let stored = record(WarmImageState::Staging, true);
    images
        .save(&stored)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let worker = Arc::new(ImageWorker::unavailable());
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        Arc::clone(&images),
        hot,
        worker.clone(),
    );

    assert!(manager.ensure_warm_consistent(&config).await.is_err());
    assert!(worker.actions().is_empty());
    assert_eq!(
        images
            .load(&stored.profile)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(stored)
    );
}

/// RUN-520: the interrupted-promotion arm must not clone over a live image, and
/// must never leave the record claiming the generation that was never promoted.
#[tokio::test]
async fn an_interrupted_promotion_never_restores_over_the_live_image() {
    let interrupted = reconcile(
        record(WarmImageState::Staging, true),
        &[
            "project-warm",
            "project-warm.staging",
            "project-warm.previous",
        ],
    )
    .await;
    assert!(
        !interrupted
            .actions
            .iter()
            .any(|action| action.starts_with("clone")),
        "{:?}",
        interrupted.actions
    );
    assert_eq!(
        interrupted
            .record
            .map(|record| (record.state, record.generation, record.base_fingerprint)),
        Some((WarmImageState::Promoted, 4, fingerprint("9900")))
    );
}

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

/// Recovery adopts only what it can prove clean, and the recycle gate is the
/// proof. This worker's gate always refuses, which is exactly the case a
/// machine that cannot be verified must fall into.
async fn recovered_hot(
    present: &[&str],
    config: Arc<flanforge_core::Config>,
) -> (Option<HotGuest>, Vec<String>) {
    reconciled_hot(
        hot_guest("ci-hot-project-0123456789ab"),
        config,
        Arc::new(ImageWorker::new(present)),
    )
    .await
}

/// Reconciles one seeded record against one backend and reports what survived.
async fn reconciled_hot(
    guest: HotGuest,
    config: Arc<flanforge_core::Config>,
    worker: Arc<ImageWorker>,
) -> (Option<HotGuest>, Vec<String>) {
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
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    hot.save(&guest)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let manager = AllocationManager::new(
        ConfigHandle::new(config),
        store,
        images,
        Arc::clone(&hot),
        worker.clone(),
    );
    manager
        .recover()
        .await
        .unwrap_or_else(|error| unreachable!("recover: {error}"));
    (
        hot.load(&guest.vm_name)
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        worker.actions(),
    )
}

/// A machine the host still reports is not a machine anyone proved clean. It
/// is retired for the same reason recovery never promotes a staged warm
/// candidate: it cannot know what state the interruption left behind.
#[tokio::test]
async fn a_surviving_hot_machine_the_gate_cannot_verify_is_retired() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (record, actions) = recovered_hot(
        &["ci-hot-project-0123456789ab"],
        test_support::hot_config(directory.path().to_path_buf()),
    )
    .await;
    assert!(record.is_none());
    assert_eq!(
        actions,
        vec![
            "hot-reset ci-hot-project-0123456789ab".to_owned(),
            "hot-evict ci-hot-project-0123456789ab".to_owned(),
        ],
        "the gate runs first, and only its refusal destroys the machine"
    );
}

/// The Tart restart story: `tart run` is a child of `flanforged`, so the VM
/// dies with the daemon and the record is all that survives.
#[tokio::test]
async fn a_hot_record_whose_machine_is_gone_is_retired_without_asking_the_backend() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (record, actions) = recovered_hot(
        &[],
        test_support::hot_config(directory.path().to_path_buf()),
    )
    .await;
    assert!(record.is_none());
    // Nothing to destroy, so asking a backend to destroy it would turn "the
    // machine is gone" into a failure and keep the record forever.
    assert!(actions.is_empty(), "{actions:?}");
}

/// A machine an operator drained does not come back claimable: `claimable` does
/// not read `drain_reason`, so a record that returned `Idle` would serve a job
/// until the next sweep noticed. The gate passing is not the question here.
#[tokio::test]
async fn recovery_does_not_return_a_drained_machine_to_the_pool() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let name = "ci-hot-project-0123456789ab";
    let mut drained = hot_guest(name);
    drained.drain_reason = Some(flanforge_core::HotDrainReason::OperatorRequest);
    let (record, actions) = reconciled_hot(
        drained,
        test_support::hot_config(directory.path().to_path_buf()),
        Arc::new(ImageWorker::gating(&[name])),
    )
    .await;
    assert!(record.is_none(), "a drained machine was adopted back");
    assert_eq!(
        actions,
        vec![format!("hot-evict {name}")],
        "the drain is the answer, so the gate is not even asked"
    );
}

/// `Draining` has no edge back to `Idle`. Writing one past the state machine is
/// what let a machine on its way out serve another job.
#[tokio::test]
async fn recovery_never_writes_a_state_the_machine_has_no_edge_to() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let name = "ci-hot-project-0123456789ab";
    let mut draining = hot_guest(name);
    draining.state = HotState::Draining;
    let (record, actions) = reconciled_hot(
        draining,
        test_support::hot_config(directory.path().to_path_buf()),
        Arc::new(ImageWorker::gating(&[name])),
    )
    .await;
    assert!(record.is_none(), "a draining machine rejoined the pool");
    assert!(
        actions.contains(&format!("hot-evict {name}")),
        "{actions:?}"
    );
}
