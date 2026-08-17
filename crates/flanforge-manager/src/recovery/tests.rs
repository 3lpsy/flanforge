use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, BaseFingerprint, Config, Profile, VmName, WarmGeneration, WarmImageRecord,
    WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::super::{
    AllocationManager, AllocationReporter, AllocationWorker, ConfigHandle, HostMachine,
    MachineState, ReapRequest, WorkerError,
};

/// Records the image operations reconciliation performs, in order.
#[derive(Debug)]
struct ImageWorker {
    machines: Vec<HostMachine>,
    actions: Mutex<Vec<String>>,
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
                })
                .collect(),
            actions: Mutex::new(Vec::new()),
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

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(self.machines.clone())
    }

    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        self.record(format!("delete {}", request.name));
        Ok(())
    }

    /// `tart clone` refuses an existing destination, so the fake does too.
    async fn clone_image(
        &self,
        source: &VmName,
        destination: &VmName,
        _profile: &Profile,
    ) -> Result<(), WorkerError> {
        self.record(format!("clone {source} {destination}"));
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
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
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
