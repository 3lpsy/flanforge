use std::sync::Arc;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource, Config,
    FallbackReason, Profile, RequestOptions, RunnerLabel, VmName, WarmGeneration, WarmImageRecord,
    WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, HotGuestStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    super::{
        AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
        HostMachine, MachineOwnership, MachineState, ManagerError, WorkerError,
    },
    retention_plan,
};

use crate::tests::UnavailableWarmImageStore;
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

/// A worker that answers the host questions selection asks, and nothing else.
/// It counts its listings, so a redundant one is visible.
#[derive(Debug, Default)]
struct HostWorker {
    machines: Vec<HostMachine>,
    fingerprint: Option<BaseFingerprint>,
    listings: Arc<std::sync::atomic::AtomicUsize>,
}

impl HostWorker {
    fn new(machines: Vec<HostMachine>, fingerprint: Option<BaseFingerprint>) -> Self {
        Self {
            machines,
            fingerprint,
            listings: Arc::default(),
        }
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
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        self.listings
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(self.machines.clone())
    }

    async fn base_fingerprint(&self, _template: &VmName) -> Option<BaseFingerprint> {
        self.fingerprint.clone()
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

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Err(WorkerError::new("host inventory unavailable"))
    }
}

fn fingerprint(value: &str) -> BaseFingerprint {
    BaseFingerprint::new(value).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn stopped(name: &str) -> HostMachine {
    HostMachine {
        state: MachineState::Stopped,
        ..running(name)
    }
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

fn record(warm_template: &str, base: &str, generation: u64) -> WarmImageRecord {
    WarmImageRecord {
        profile: test_support::profile_name(),
        warm_template: VmName::new(warm_template)
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        generation,
        base_fingerprint: fingerprint(base),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1_755_324_251,
        state: WarmImageState::Promoted,
        previous: None,
    }
}

/// The half-written promotion: the record names the new generation and the new
/// base before the image under the name is either.
fn staging(record: WarmImageRecord) -> WarmImageRecord {
    WarmImageRecord {
        state: WarmImageState::Staging,
        ..record
    }
}

struct Fixture {
    manager: AllocationManager,
    images: Arc<dyn WarmImageStore>,
    _directory: tempfile::TempDir,
}

async fn fixture(config: Arc<Config>, worker: impl AllocationWorker + 'static) -> Fixture {
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
    Fixture {
        manager: AllocationManager::new(
            ConfigHandle::new(config),
            store,
            images.clone(),
            hot,
            Arc::new(worker),
        ),
        images,
        _directory: directory,
    }
}

async fn fixture_with_image_store(
    config: Arc<Config>,
    worker: impl AllocationWorker + 'static,
    images: Arc<dyn WarmImageStore>,
) -> Fixture {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot: Arc<dyn HotGuestStore> = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    Fixture {
        manager: AllocationManager::new(
            ConfigHandle::new(config),
            store,
            Arc::clone(&images),
            hot,
            Arc::new(worker),
        ),
        images,
        _directory: directory,
    }
}

fn warm_profile(config: &Config) -> Profile {
    config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!("fixture: profile"))
}

async fn selection(
    worker: impl AllocationWorker + 'static,
    stored: Option<WarmImageRecord>,
) -> super::source::SourceSelection {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), worker).await;
    if let Some(record) = stored {
        fixture
            .images
            .save(&record)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    fixture
        .manager
        .resolve_source(
            &test_support::profile_name(),
            &warm_profile(&config),
            AllocationMode::Warm,
        )
        .await
}

#[tokio::test]
async fn source_selection_falls_back_for_every_enumerated_reason() {
    let cold = test_support::config("/tmp/flanforge-warm".into());
    let undeclared = fixture(Arc::clone(&cold), HostWorker::default()).await;
    assert_eq!(
        undeclared
            .manager
            .resolve_source(
                &test_support::profile_name(),
                &warm_profile(&cold),
                AllocationMode::Warm,
            )
            .await
            .fallback_reason,
        Some(FallbackReason::NotDeclared)
    );

    let warm = HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01")));
    for (worker, stored, expected) in [
        (
            HostWorker::new(Vec::new(), Some(fingerprint("aa01"))),
            None,
            FallbackReason::NoRecord,
        ),
        (
            HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
            None,
            FallbackReason::Unclaimed,
        ),
        (
            HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
            Some(record("project-elsewhere", "aa01", 1)),
            FallbackReason::Repointed,
        ),
        (
            HostWorker::new(Vec::new(), Some(fingerprint("aa01"))),
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::Absent,
        ),
        (
            HostWorker::new(vec![running("project-warm")], Some(fingerprint("aa01"))),
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::NotStopped,
        ),
        (
            HostWorker::new(vec![stopped("project-warm")], None),
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::FingerprintUnavailable,
        ),
        (
            HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("bb02"))),
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::StaleBase,
        ),
        (
            HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
            Some(staging(record("project-warm", "aa01", 2))),
            FallbackReason::NotPromoted,
        ),
    ] {
        let source = selection(worker, stored).await;
        assert_eq!(source.fallback_reason, Some(expected));
        assert_eq!(source.kind, CloneKind::Template);
    }

    let promoted = selection(warm, Some(record("project-warm", "aa01", 1))).await;
    assert_eq!(promoted.kind, CloneKind::Warm);
    assert_eq!(promoted.fallback_reason, None);
    assert_eq!(promoted.warm_generation, Some(1));
}

#[tokio::test]
async fn source_selection_falls_back_when_image_authority_is_unavailable() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture_with_image_store(
        Arc::clone(&config),
        HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
        Arc::new(UnavailableWarmImageStore),
    )
    .await;

    let source = fixture
        .manager
        .resolve_source(
            &test_support::profile_name(),
            &warm_profile(&config),
            AllocationMode::Warm,
        )
        .await;
    assert_eq!(source.kind, CloneKind::Template);
    assert_eq!(source.fallback_reason, Some(FallbackReason::Unclaimed));
}

#[tokio::test]
async fn source_selection_falls_back_when_host_inventory_is_unavailable() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), UnavailableHostWorker).await;
    fixture
        .images
        .save(&record("project-warm", "aa01", 1))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let source = fixture
        .manager
        .resolve_source(
            &test_support::profile_name(),
            &warm_profile(&config),
            AllocationMode::Warm,
        )
        .await;
    assert_eq!(source.kind, CloneKind::Template);
    assert_eq!(source.fallback_reason, Some(FallbackReason::Absent));
}

#[tokio::test]
async fn a_warm_boot_failure_quarantines_the_image_and_the_next_allocation_is_cold() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
    )
    .await;
    fixture
        .images
        .save(&record("project-warm", "aa01", 1))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    fixture
        .manager
        .quarantine_warm(&test_support::profile_name(), "boot failed")
        .await;

    assert!(
        fixture
            .manager
            .is_warm_quarantined(&test_support::profile_name())
            .await
    );
    assert_eq!(
        fixture
            .manager
            .resolve_source(
                &test_support::profile_name(),
                &warm_profile(&config),
                AllocationMode::Warm,
            )
            .await
            .fallback_reason,
        Some(FallbackReason::Quarantined)
    );
}

#[tokio::test]
async fn regeneration_ignores_a_warm_request() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
    )
    .await;
    fixture
        .images
        .save(&record("project-warm", "aa01", 1))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let profile = warm_profile(&config);
    let source = fixture
        .manager
        .resolve_source(
            &test_support::profile_name(),
            &profile,
            AllocationMode::Regenerate,
        )
        .await;
    assert_eq!(source.name, profile.template);
    assert_eq!(source.kind, CloneKind::Template);
}

/// The verified workflow claim decides the mode, never the request body.
#[tokio::test]
async fn a_regeneration_workflow_produces_a_regenerate_allocation_that_asked_for_warm() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    config.runtime.state_dir = directory.path().to_path_buf();
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
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::new(config)),
        store,
        images,
        hot,
        Arc::new(HostWorker::default()),
    );

    let created = manager
        .create(
            test_support::request(),
            RequestOptions {
                warm: true,
                hot: flanforge_core::HotRequest::Untouched,
                ..RequestOptions::default()
            },
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(created.allocation.mode, AllocationMode::Regenerate);
    assert_eq!(
        created.allocation.source.map(|source| source.name),
        Some(test_support::profile().template)
    );
}

fn regeneration(config: &Config) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-retain")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Regenerate,
        test_support::size(),
    );
    allocation.state = AllocationState::Completed;
    allocation.set_vm_created();
    allocation.set_source(CloneSource {
        name: warm_profile(config).template,
        kind: CloneKind::Template,
        base_fingerprint: Some(fingerprint("aa01")),
        fallback_reason: None,
    });
    allocation
}

#[tokio::test]
async fn retention_is_refused_unless_the_profile_still_declares_production() {
    let declared = test_support::warm_config("/tmp/flanforge-warm".into());
    let declaring = fixture(Arc::clone(&declared), HostWorker::default()).await;
    let allocation = regeneration(&declared);
    let plan = retention_plan(&declaring.manager, &allocation).await;
    let plan = plan.unwrap_or_else(|error| unreachable!("plan: {error}"));
    assert_eq!(
        plan.map(|plan| (plan.warm_template.to_string(), plan.generation)),
        Some(("project-warm".to_owned(), 1))
    );

    // The same allocation against a profile that never declared production.
    let bare = fixture(
        test_support::config("/tmp/flanforge-warm".into()),
        HostWorker::default(),
    )
    .await;
    assert_eq!(
        retention_plan(&bare.manager, &allocation)
            .await
            .unwrap_or_else(|error| unreachable!("plan: {error}")),
        None
    );
}

#[tokio::test]
async fn retention_propagates_an_unavailable_image_store() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture_with_image_store(
        Arc::clone(&config),
        HostWorker::default(),
        Arc::new(UnavailableWarmImageStore),
    )
    .await;

    assert!(matches!(
        retention_plan(&fixture.manager, &regeneration(&config)).await,
        Err(ManagerError::Store(_))
    ));
}

#[tokio::test]
async fn retention_propagates_an_unavailable_host_inventory() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), UnavailableHostWorker).await;

    assert!(matches!(
        retention_plan(&fixture.manager, &regeneration(&config)).await,
        Err(ManagerError::Worker(_))
    ));
}

#[tokio::test]
async fn retention_re_reads_the_profile_so_a_withdrawn_declaration_stops_it() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), HostWorker::default()).await;
    let allocation = regeneration(&config);
    assert!(
        retention_plan(&fixture.manager, &allocation)
            .await
            .unwrap_or_else(|error| unreachable!("plan: {error}"))
            .is_some()
    );

    let mut withdrawn = (*config).clone();
    for profile in withdrawn.profiles.values_mut() {
        profile.warm_template = None;
        profile.regeneration_workflow = None;
    }
    fixture
        .manager
        .config_handle()
        .apply(Arc::new(withdrawn))
        .unwrap_or_else(|error| unreachable!("reload: {error}"));

    assert_eq!(
        retention_plan(&fixture.manager, &allocation)
            .await
            .unwrap_or_else(|error| unreachable!("plan: {error}")),
        None
    );
}

#[tokio::test]
async fn an_unclaimed_image_is_neither_used_nor_overwritten() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker::new(vec![stopped("project-warm")], Some(fingerprint("aa01"))),
    )
    .await;

    assert_eq!(
        fixture
            .manager
            .resolve_source(
                &test_support::profile_name(),
                &warm_profile(&config),
                AllocationMode::Warm,
            )
            .await
            .fallback_reason,
        Some(FallbackReason::Unclaimed)
    );
    assert_eq!(
        retention_plan(&fixture.manager, &regeneration(&config))
            .await
            .unwrap_or_else(|error| unreachable!("plan: {error}")),
        None
    );
}

#[tokio::test]
async fn a_regeneration_from_a_warm_source_is_refused_so_lineage_stays_one_deep() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), HostWorker::default()).await;
    let mut allocation = regeneration(&config);
    allocation.set_source(CloneSource {
        name: VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        kind: CloneKind::Warm,
        base_fingerprint: Some(fingerprint("aa01")),
        fallback_reason: None,
    });

    assert_eq!(
        retention_plan(&fixture.manager, &allocation)
            .await
            .unwrap_or_else(|error| unreachable!("plan: {error}")),
        None
    );
}

#[tokio::test]
async fn exactly_one_previous_generation_survives_a_promotion() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), HostWorker::default()).await;
    let mut stored = record("project-warm", "aa01", 7);
    stored.previous = Some(WarmGeneration {
        generation: 6,
        base_fingerprint: fingerprint("9900"),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1,
    });
    fixture
        .images
        .save(&stored)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let plan = retention_plan(&fixture.manager, &regeneration(&config))
        .await
        .unwrap_or_else(|error| unreachable!("plan: {error}"))
        .unwrap_or_else(|| unreachable!("plan"));
    assert_eq!(plan.generation, 8);
    // The plan carries generation 7 alone: generation 6 is dropped, so exactly
    // one rollback generation survives.
    assert_eq!(
        plan.previous.map(|previous| previous.generation),
        Some(stored.generation)
    );
}

/// The shared backend contract. A name-addressed backend answers selection
/// from its VM listing and a pointer-addressed one from its own durable
/// document; both must reach the same arm for the same situation, or the
/// manager's warm/cold decision means something different per backend.
#[derive(Clone, Copy, Debug)]
enum Availability {
    Ready,
    Busy,
    Absent,
}

/// The pointer-addressed backend: no image is name-addressed, so nothing can
/// be `Unclaimed`.
#[derive(Debug)]
struct PointerWorker {
    availability: Availability,
    fingerprint: Option<BaseFingerprint>,
    capabilities: flanforge_wire::RuntimeCapabilities,
}

impl PointerWorker {
    fn new(availability: Availability, fingerprint: Option<BaseFingerprint>) -> Self {
        Self {
            availability,
            fingerprint,
            capabilities: flanforge_wire::RuntimeCapabilities::libvirt(),
        }
    }

    /// The historical libvirt set, as an older backend build still declares
    /// it. Decoded rather than constructed, because a capability set is only
    /// ever canonical.
    fn without_warm_images() -> Self {
        let document = r#"{"backend":"libvirt","capabilities":["image_runner","resource_inventory"],"health":"healthy","message":null}"#;
        let status = serde_json::from_str::<flanforge_wire::RuntimeStatus>(document)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        Self {
            availability: Availability::Ready,
            fingerprint: Some(fingerprint("aa01")),
            capabilities: status.capabilities().clone(),
        }
    }
}

#[async_trait]
impl AllocationWorker for PointerWorker {
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

    fn capabilities(&self) -> flanforge_wire::RuntimeCapabilities {
        self.capabilities.clone()
    }

    async fn base_fingerprint(&self, _template: &VmName) -> Option<BaseFingerprint> {
        self.fingerprint.clone()
    }

    async fn is_unclaimed_image(
        &self,
        _name: &VmName,
        _listing: &[HostMachine],
    ) -> Result<bool, WorkerError> {
        Ok(false)
    }

    async fn warm_availability(
        &self,
        _profile: &Profile,
        record: &WarmImageRecord,
        _listing: &[HostMachine],
    ) -> Result<crate::WarmAvailability, WorkerError> {
        if record.warm_template.as_str() != "project-warm" {
            return Ok(crate::WarmAvailability::Absent);
        }
        Ok(match self.availability {
            Availability::Ready => crate::WarmAvailability::Ready,
            Availability::Busy => crate::WarmAvailability::Busy,
            Availability::Absent => crate::WarmAvailability::Absent,
        })
    }
}

fn host_worker(availability: Availability, fingerprint: Option<BaseFingerprint>) -> HostWorker {
    let machines = match availability {
        Availability::Ready => vec![stopped("project-warm")],
        Availability::Busy => vec![running("project-warm")],
        Availability::Absent => Vec::new(),
    };
    HostWorker::new(machines, fingerprint)
}

/// One availability, one arm — whichever backend answers it. The two fakes
/// stand in for a name-addressed and a pointer-addressed backend, so this
/// pins the manager's half of the contract; the libvirt half is asserted
/// against its real implementation in that crate's `warm` tests.
#[tokio::test]
async fn one_availability_reaches_one_arm_whichever_backend_answers_it() {
    let known = || Some(fingerprint("aa01"));
    let table = [
        (
            Availability::Ready,
            known(),
            Some(record("project-elsewhere", "aa01", 1)),
            Some(FallbackReason::Repointed),
        ),
        (
            Availability::Ready,
            known(),
            Some(staging(record("project-warm", "aa01", 2))),
            Some(FallbackReason::NotPromoted),
        ),
        (
            Availability::Absent,
            known(),
            Some(record("project-warm", "aa01", 1)),
            Some(FallbackReason::Absent),
        ),
        (
            Availability::Busy,
            known(),
            Some(record("project-warm", "aa01", 1)),
            Some(FallbackReason::NotStopped),
        ),
        (
            Availability::Ready,
            None,
            Some(record("project-warm", "aa01", 1)),
            Some(FallbackReason::FingerprintUnavailable),
        ),
        (
            Availability::Ready,
            Some(fingerprint("bb02")),
            Some(record("project-warm", "aa01", 1)),
            Some(FallbackReason::StaleBase),
        ),
        (
            Availability::Ready,
            known(),
            Some(record("project-warm", "aa01", 1)),
            None,
        ),
    ];
    for (availability, print, stored, expected) in table {
        let named = selection(host_worker(availability, print.clone()), stored.clone()).await;
        let pointed = selection(PointerWorker::new(availability, print), stored).await;
        assert_eq!(named.fallback_reason, expected, "{availability:?}");
        assert_eq!(
            pointed.fallback_reason, expected,
            "backends disagree on {availability:?}"
        );
        assert_eq!(named.kind, pointed.kind);
    }
}

/// `Unclaimed` means an image sits under the declared name that the daemon
/// never recorded. Only a backend answering `is_unclaimed_image` with true can
/// produce it; one that answers false reports `NoRecord` for the same
/// situation, which is what libvirt's override does.
#[tokio::test]
async fn only_a_backend_that_claims_an_image_exists_reports_it_unclaimed() {
    let named = selection(
        host_worker(Availability::Ready, Some(fingerprint("aa01"))),
        None,
    )
    .await;
    assert_eq!(named.fallback_reason, Some(FallbackReason::Unclaimed));

    let pointed = selection(
        PointerWorker::new(Availability::Ready, Some(fingerprint("aa01"))),
        None,
    )
    .await;
    assert_eq!(pointed.fallback_reason, Some(FallbackReason::NoRecord));
}

/// Lifting the configuration refusal must not leave warm production ungated:
/// the capability set is the runtime gate, and this is its only call site.
#[tokio::test]
async fn a_backend_without_the_warm_capability_never_boots_warm() {
    let source = selection(
        PointerWorker::without_warm_images(),
        Some(record("project-warm", "aa01", 1)),
    )
    .await;
    assert_eq!(source.kind, CloneKind::Template);
    assert_eq!(source.fallback_reason, Some(FallbackReason::Unsupported));
}

/// A promotion that stops after the staging record is written must leave the
/// record describing the generation that actually survives: left in `Staging`
/// it costs every later allocation a cold boot, and only a restart repairs it.
#[tokio::test]
async fn an_abandoned_promotion_reverts_the_record_to_the_surviving_generation() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), HostWorker::default()).await;
    let warm = VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let previous = WarmGeneration {
        generation: 7,
        base_fingerprint: fingerprint("9900"),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1_755_000_000,
    };
    let mut staged = staging(record("project-warm", "aa01", 8));
    staged.previous = Some(previous.clone());
    fixture
        .images
        .save(&staged)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    fixture
        .manager
        .ensure_warm_reverted(&test_support::profile_name(), &warm, Some(&previous))
        .await
        .unwrap_or_else(|error| unreachable!("revert: {error}"));
    let reverted = fixture
        .images
        .load(&test_support::profile_name())
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"))
        .unwrap_or_else(|| unreachable!("record"));
    assert_eq!(reverted.generation, 7);
    assert_eq!(reverted.state, WarmImageState::Promoted);
    assert_eq!(reverted.base_fingerprint, fingerprint("9900"));
    assert_eq!(
        reverted.previous, None,
        "exactly one rollback generation survives"
    );

    // A first generation has nothing to fall back to, so the record goes.
    fixture
        .manager
        .ensure_warm_reverted(&test_support::profile_name(), &warm, None)
        .await
        .unwrap_or_else(|error| unreachable!("revert: {error}"));
    assert_eq!(
        fixture
            .images
            .load(&test_support::profile_name())
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        None
    );
}

/// Selection answers from the listing admission already cached rather than
/// listing the host itself: on a name-addressed backend that listing is a
/// subprocess, and one per create turns a job matrix into a listing storm.
#[tokio::test]
async fn a_warm_selection_reuses_the_cached_host_listing() {
    let mut config = (*test_support::warm_config("/tmp/flanforge-warm".into())).clone();
    // Long enough that the assertion is about reuse rather than about timing.
    config.runtime.poll_seconds = 600;
    let config = Arc::new(config);
    let listings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker {
            machines: vec![stopped("project-warm")],
            fingerprint: Some(fingerprint("aa01")),
            listings: Arc::clone(&listings),
        },
    )
    .await;
    fixture
        .images
        .save(&record("project-warm", "aa01", 1))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    for _ in 0..3 {
        let source = fixture
            .manager
            .resolve_source(
                &test_support::profile_name(),
                &warm_profile(&config),
                AllocationMode::Warm,
            )
            .await;
        assert_eq!(source.kind, CloneKind::Warm);
    }
    assert_eq!(
        listings.load(std::sync::atomic::Ordering::Acquire),
        1,
        "one listing answered every selection"
    );
}
