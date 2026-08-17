use std::sync::Arc;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource, Config,
    FallbackReason, Profile, RequestOptions, RunnerLabel, VmName, WarmGeneration, WarmImageRecord,
    WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    super::{
        AllocationManager, AllocationReporter, AllocationWorker, ConfigHandle, HostMachine,
        MachineState, WorkerError,
    },
    retention_plan,
};

/// A worker that answers the host questions selection asks, and nothing else.
#[derive(Debug, Default)]
struct HostWorker {
    machines: Vec<HostMachine>,
    fingerprint: Option<BaseFingerprint>,
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

    async fn base_fingerprint(&self, _template: &VmName) -> Option<BaseFingerprint> {
        self.fingerprint.clone()
    }
}

fn fingerprint(value: &str) -> BaseFingerprint {
    BaseFingerprint::new(value).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn stopped(name: &str) -> HostMachine {
    HostMachine {
        name: name.to_owned(),
        state: MachineState::Stopped,
        age_seconds: Some(10),
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

async fn fixture(config: Arc<Config>, worker: HostWorker) -> Fixture {
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
    Fixture {
        manager: AllocationManager::new(
            ConfigHandle::new(config),
            store,
            images.clone(),
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

async fn selection(worker: HostWorker, stored: Option<WarmImageRecord>) -> CloneSource {
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

    let warm = HostWorker {
        machines: vec![stopped("project-warm")],
        fingerprint: Some(fingerprint("aa01")),
    };
    for (worker, stored, expected) in [
        (
            HostWorker {
                machines: Vec::new(),
                fingerprint: Some(fingerprint("aa01")),
            },
            None,
            FallbackReason::NoRecord,
        ),
        (
            HostWorker {
                machines: vec![stopped("project-warm")],
                fingerprint: Some(fingerprint("aa01")),
            },
            None,
            FallbackReason::Unclaimed,
        ),
        (
            HostWorker {
                machines: vec![stopped("project-warm")],
                fingerprint: Some(fingerprint("aa01")),
            },
            Some(record("project-elsewhere", "aa01", 1)),
            FallbackReason::Repointed,
        ),
        (
            HostWorker {
                machines: Vec::new(),
                fingerprint: Some(fingerprint("aa01")),
            },
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::Absent,
        ),
        (
            HostWorker {
                machines: vec![HostMachine {
                    name: "project-warm".to_owned(),
                    state: MachineState::Running,
                    age_seconds: Some(10),
                }],
                fingerprint: Some(fingerprint("aa01")),
            },
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::NotStopped,
        ),
        (
            HostWorker {
                machines: vec![stopped("project-warm")],
                fingerprint: None,
            },
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::FingerprintUnavailable,
        ),
        (
            HostWorker {
                machines: vec![stopped("project-warm")],
                fingerprint: Some(fingerprint("bb02")),
            },
            Some(record("project-warm", "aa01", 1)),
            FallbackReason::StaleBase,
        ),
        (
            HostWorker {
                machines: vec![stopped("project-warm")],
                fingerprint: Some(fingerprint("aa01")),
            },
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
}

#[tokio::test]
async fn a_warm_boot_failure_quarantines_the_image_and_the_next_allocation_is_cold() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker {
            machines: vec![stopped("project-warm")],
            fingerprint: Some(fingerprint("aa01")),
        },
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
        HostWorker {
            machines: vec![stopped("project-warm")],
            fingerprint: Some(fingerprint("aa01")),
        },
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
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::new(config)),
        store,
        images,
        Arc::new(HostWorker::default()),
    );

    let created = manager
        .create(
            test_support::request(),
            RequestOptions {
                warm: true,
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
    assert_eq!(retention_plan(&bare.manager, &allocation).await, None);
}

#[tokio::test]
async fn retention_re_reads_the_profile_so_a_withdrawn_declaration_stops_it() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(Arc::clone(&config), HostWorker::default()).await;
    let allocation = regeneration(&config);
    assert!(
        retention_plan(&fixture.manager, &allocation)
            .await
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

    assert_eq!(retention_plan(&fixture.manager, &allocation).await, None);
}

#[tokio::test]
async fn an_unclaimed_image_is_neither_used_nor_overwritten() {
    let config = test_support::warm_config("/tmp/flanforge-warm".into());
    let fixture = fixture(
        Arc::clone(&config),
        HostWorker {
            machines: vec![stopped("project-warm")],
            fingerprint: Some(fingerprint("aa01")),
        },
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
        retention_plan(&fixture.manager, &regeneration(&config)).await,
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

    assert_eq!(retention_plan(&fixture.manager, &allocation).await, None);
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
        .unwrap_or_else(|| unreachable!("plan"));
    assert_eq!(plan.generation, 8);
    // The plan carries generation 7 alone: generation 6 is dropped, so exactly
    // one rollback generation survives.
    assert_eq!(
        plan.previous.map(|previous| previous.generation),
        Some(stored.generation)
    );
}
