use std::{
    fs::{DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use flanforge_core::{Allocation, AllocationMode, AllocationOrigin, RunnerLabel, VmName};
use flanforge_libvirt_wire::{Artifact, OwnershipManifest};
use flanforge_manager::{AllocationWorker, CleanupBudget};
use flanforge_runtime::{GuestExit, GuestOutput};
use uuid::Uuid;

use crate::manifest::{ServiceInstance, allocation_dir, cleanup_path, path, save};

use super::{super::LibvirtWorker, gate::ensure_clean};

const DOMAIN: &str = "ci-libvirt-hot-1";

/// Cleanup must leave a pool machine, and the durable authority a later
/// eviction reads, exactly where they are — whether the pool took this
/// allocation's guest or this allocation reused someone else's.
#[tokio::test]
async fn a_pooled_guest_and_its_ownership_authority_survive_cleanup() {
    let fixture = Fixture::new();
    let worker = fixture.worker();
    let vm_name = vm_name();
    let cloned = fixture.allocation(AllocationOrigin::Cloned);

    worker.hot.ensure_claimed(&vm_name).await;
    let _ = worker.cleanup(cloned.clone(), budget()).await;
    assert!(
        fixture.requests().is_empty(),
        "a guest the pool claimed was handed to the hypervisor for deletion"
    );
    fixture.assert_authority_intact();

    // A machine this allocation only borrowed is the pool's for the same
    // reason, and the claim set is not what proves it.
    worker.hot.release(&vm_name).await;
    let reused = fixture.allocation(AllocationOrigin::HotReuse {
        vm_name: vm_name.clone(),
        jobs_served: 3,
        booted_at_unix: 1_755_324_251,
    });
    let _ = worker.cleanup(reused, budget()).await;
    assert!(fixture.requests().is_empty());
    fixture.assert_authority_intact();

    // The control: with no claim and no reuse, the guest is this allocation's
    // own and cleanup destroys it.
    let _ = worker.cleanup(cloned, budget()).await;
    assert!(
        fixture
            .requests()
            .iter()
            .any(|request| request.contains("\"operation\":\"cleanup\"")),
        "an unpooled guest was never handed to the hypervisor for deletion"
    );
}

/// The gate passes on one shape only. Everything else names where the reset
/// stopped, because the guest is the one thing a hot machine's previous job
/// could have influenced.
#[test]
fn the_recycle_gate_passes_only_on_a_zero_exit_and_a_clean_verdict() {
    let clean = ensure_clean(&GuestOutput::new(
        GuestExit::Code(0),
        report(true, "clean", "clean", ""),
        false,
    ));
    assert_eq!(clean.map(|verdict| verdict.free_mb()).ok(), Some(42_000));

    for (output, expected, reason) in [
        (
            GuestOutput::new(
                GuestExit::Code(1),
                report(true, "clean", "clean", ""),
                false,
            ),
            "the recycle gate failed at clean",
            "a non-zero exit outranks a clean claim",
        ),
        (
            GuestOutput::new(
                GuestExit::Code(0),
                report(false, "disk", "low", "3 MiB free"),
                false,
            ),
            "the recycle gate failed at disk (low): 3 MiB free",
            "an unclean verdict names its gate",
        ),
        (
            GuestOutput::new(
                GuestExit::Signal(9),
                report(true, "clean", "clean", ""),
                false,
            ),
            "terminated by a signal",
            "a killed gate reached no verdict",
        ),
        (
            GuestOutput::new(GuestExit::Code(0), report(true, "clean", "clean", ""), true),
            "oversized",
            "a truncated report is a prefix, not a verdict",
        ),
        (
            GuestOutput::new(GuestExit::Code(0), b"not json".to_vec(), false),
            "(gate exit 0)",
            "a malformed report carries the exit that produced it",
        ),
    ] {
        let Err(error) = ensure_clean(&output) else {
            unreachable!("the recycle gate passed: {reason}");
        };
        assert!(error.to_string().contains(expected), "{reason}");
    }
}

fn report(is_clean: bool, gate: &str, state: &str, detail: &str) -> Vec<u8> {
    format!(
        "{{\"schema\":1,\"contract\":1,\"clean\":{is_clean},\"gate\":\"{gate}\",\
         \"state\":\"{state}\",\"detail\":\"{detail}\",\"free_mb\":42000,\"skew_seconds\":0,\
         \"job_account\":{{\"name\":\"runner\",\"uid\":1000}}}}"
    )
    .into_bytes()
}

fn vm_name() -> VmName {
    VmName::new(DOMAIN).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn budget() -> CleanupBudget {
    CleanupBudget::allow(Duration::from_secs(30))
}

/// One allocation's durable ownership authority, and a helper process that
/// records every request rather than talking to a hypervisor.
struct Fixture {
    directory: tempfile::TempDir,
    helper: PathBuf,
    allocation: Allocation,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("state: {error}"));
        let log = directory.path().join("requests.log");
        let helper = flanforge_test_support::executable(
            directory.path(),
            "helper",
            &format!(
                "cat >> '{log}'\necho >> '{log}'\nprintf '{{\"result\":\"unit\"}}'\n",
                log = log.display()
            ),
        );
        let allocation = Allocation::new(
            flanforge_test_support::request(),
            vm_name(),
            RunnerLabel::new("flanforged-1")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            AllocationMode::Cold,
            flanforge_test_support::size(),
        );
        write_authority(directory.path(), allocation.id.into_uuid());
        Self {
            directory,
            helper,
            allocation,
        }
    }

    fn worker(&self) -> LibvirtWorker {
        LibvirtWorker::with_helper(self.directory.path(), self.helper.clone())
    }

    fn allocation(&self, origin: AllocationOrigin) -> Allocation {
        let mut allocation = self.allocation.clone();
        allocation.origin = origin;
        allocation.set_vm_created();
        allocation
    }

    /// Every request the helper was handed since the last call.
    fn requests(&self) -> Vec<String> {
        let log = self.directory.path().join("requests.log");
        let Ok(contents) = std::fs::read_to_string(&log) else {
            return Vec::new();
        };
        let _ = std::fs::remove_file(&log);
        contents
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }

    fn assert_authority_intact(&self) {
        let state = self.directory.path();
        let allocation_id = self.allocation.id.into_uuid();
        assert!(path(state, allocation_id).is_file());
        assert!(
            allocation_dir(state, allocation_id)
                .join("known_hosts")
                .is_file()
        );
        assert!(!cleanup_path(state, allocation_id).exists());
    }
}

/// The ownership manifest and host-key anchor a hot guest's later claims and
/// its eviction both read.
fn write_authority(state_dir: &Path, allocation_id: Uuid) {
    let instance = ServiceInstance::load_or_create(state_dir)
        .unwrap_or_else(|error| unreachable!("instance: {error}"));
    let directory = allocation_dir(state_dir, allocation_id);
    let mut builder = DirBuilder::new();
    builder.mode(0o700).recursive(true);
    builder
        .create(&directory)
        .unwrap_or_else(|error| unreachable!("directory: {error}"));
    let artifact_id = Uuid::new_v4();
    let manifest = OwnershipManifest::new(
        allocation_id,
        instance.id(),
        DOMAIN.to_owned(),
        Uuid::new_v4(),
        "02:01:02:03:04:05".to_owned(),
        Artifact::new(format!("root-{artifact_id}.qcow2")),
        Artifact::new(format!("seed-{artifact_id}.img")),
        "flanforge".to_owned(),
        directory.join("known_hosts"),
        1,
    );
    save(&manifest, &path(state_dir, allocation_id))
        .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let mut known_hosts = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("known_hosts"))
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));
    known_hosts
        .write_all(b"flanforge-guest ssh-ed25519 AAAA\n")
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));
    known_hosts
        .sync_all()
        .unwrap_or_else(|error| unreachable!("known hosts: {error}"));
}

/// RUN-746: the gate resolves profiles from the live reload feed, so a
/// profile added after `open` claims its pool machines instead of failing
/// them into eviction.
#[tokio::test]
async fn the_recycle_gate_sees_a_profile_added_by_a_reload() {
    let fixture = Fixture::new();
    let (worker, reloads) =
        LibvirtWorker::with_helper_watching(fixture.directory.path(), fixture.helper.clone());
    let added = flanforge_core::ProfileName::new("added-later")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(worker.hot_template(&added).is_err());

    let mut updated = flanforge_test_support::config(fixture.directory.path().to_path_buf())
        .as_ref()
        .clone();
    let mut profile = flanforge_test_support::profile();
    profile.template =
        VmName::new("ci-added-template").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    updated.profiles.insert(added.clone(), profile);
    reloads.send_replace(std::sync::Arc::new(updated));

    let template = worker
        .hot_template(&added)
        .unwrap_or_else(|error| unreachable!("template: {error}"));
    assert_eq!(template.as_str(), "ci-added-template");
}
