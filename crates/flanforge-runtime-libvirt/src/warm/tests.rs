use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use flanforge_core::{BaseFingerprint, ProfileName, VmName, WarmImageRecord, WarmImageState};
use flanforge_libvirt_wire::{
    CheckpointVolumeRole, PublishedWarm, VolumeCheckpoint, VolumePointer,
};
use flanforge_manager::{AllocationWorker, ImageSweep, WarmAvailability};
use flanforge_runtime::{REGENERATION_SENTINEL, retention_marker_script};
use uuid::Uuid;

use crate::{checkpoint::warm_capture_id, worker::LibvirtWorker};

use super::{
    ensure_published, find_verify_command_for_test, generalization_scripts, identity_paths, load,
    load_all, path, remove,
    retire::{WARM_RETIREMENT_AGE_SECONDS, candidates, is_old_enough},
};

const GIB: u64 = 1_024 * 1_024 * 1_024;

fn profile() -> ProfileName {
    ProfileName::new("ci").unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn capture(generation: u64) -> (Uuid, VolumePointer) {
    let capture_id = Uuid::from_u128(u128::from(generation) + 1);
    let name = VolumeCheckpoint::warm_volume_name(capture_id);
    let pointer = VolumePointer::new(
        "flanforge".to_owned(),
        name.clone(),
        format!("/pool/{name}"),
        40 * GIB,
        generation,
        1_755_324_251 + generation,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    (capture_id, pointer)
}

fn document(generation: u64) -> PublishedWarm {
    let (_, pointer) = capture(generation);
    PublishedWarm::new(
        profile().to_string(),
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        pointer.produced_at_unix(),
        pointer,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn state_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

#[test]
fn a_pointer_round_trips_through_an_atomic_private_replace() {
    let root = state_dir();
    let first = document(1);
    ensure_published(root.path(), &first).unwrap_or_else(|error| unreachable!("publish: {error}"));
    assert_eq!(
        load(root.path(), &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(first.clone())
    );
    let file = path(root.path(), &profile());
    let mode = std::fs::metadata(&file)
        .unwrap_or_else(|error| unreachable!("metadata: {error}"))
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "warm pointers stay private");

    // Repointing is a replace, unlike the write-once cold publication.
    let (_, next) = capture(2);
    let second = first
        .repointed(
            "flanforge-warm".to_owned(),
            Uuid::new_v4(),
            next.produced_at_unix(),
            next,
        )
        .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    ensure_published(root.path(), &second).unwrap_or_else(|error| unreachable!("publish: {error}"));
    assert_eq!(
        load(root.path(), &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(second)
    );

    remove(root.path(), &profile()).unwrap_or_else(|error| unreachable!("remove: {error}"));
    assert_eq!(
        load(root.path(), &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        None
    );
}

/// Fail-closed exactly as the warm image store is: an unreadable pointer costs
/// a cold boot rather than wedging the daemon.
#[test]
fn an_unreadable_or_mismatched_pointer_is_quarantined_and_read_as_absent() {
    let root = state_dir();
    ensure_published(root.path(), &document(1))
        .unwrap_or_else(|error| unreachable!("publish: {error}"));
    let file = path(root.path(), &profile());
    write_private(&file, b"{ not json");
    assert_eq!(
        load(root.path(), &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        None
    );
    assert!(!file.exists(), "the corrupt file was quarantined");
    assert!(file.with_extension("json.corrupt").exists());

    // A document naming another profile is quarantined too: the filename is
    // the binding, and a pointer that disagrees with it authorizes nothing.
    let other = PublishedWarm::new(
        "other".to_owned(),
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        1_755_324_252,
        capture(1).1,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let bytes = other
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    write_private(&file, &bytes);
    assert_eq!(
        load(root.path(), &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        None
    );
}

#[test]
fn a_filename_that_does_not_round_trip_names_no_profile() {
    let root = state_dir();
    ensure_published(root.path(), &document(1))
        .unwrap_or_else(|error| unreachable!("publish: {error}"));
    let directory = super::directory(root.path());
    write_private(&directory.join("Not A Profile.warm.json"), b"{}");
    write_private(&directory.join("stray.json"), b"{}");
    let documents = load_all(root.path()).unwrap_or_else(|error| unreachable!("load_all: {error}"));
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].profile(), "ci");
}

/// The crash matrix. Promotion is purely additive, so at every boundary before
/// the pointer publish a pre-existing consumer still resolves to the exact
/// generation it booted from, and the candidate is never pointer-referenced.
#[test]
fn every_crash_boundary_leaves_the_live_generation_resolvable() {
    let root = state_dir();
    let live = document(1);
    ensure_published(root.path(), &live).unwrap_or_else(|error| unreachable!("publish: {error}"));
    let booted = live.current().volume_key().to_owned();
    let (capture_id, candidate) = capture(2);

    for boundary in [Boundary::Staged, Boundary::Captured, Boundary::Verified] {
        write_checkpoint(root.path(), capture_id, boundary);
        let current = load(root.path(), &profile())
            .unwrap_or_else(|error| unreachable!("load: {error}"))
            .unwrap_or_else(|| unreachable!("pointer"));
        assert_eq!(current.current().volume_key(), booted, "{boundary:?}");
        assert!(
            !current
                .pointers()
                .iter()
                .any(|pointer| pointer.is_same_volume(&candidate)),
            "an unpublished generation is referenced by nothing, by construction"
        );
    }

    // After the publish the candidate is current and the outgoing generation
    // is retained, so nothing a consumer holds open has been replaced.
    let promoted = live
        .repointed(
            "flanforge-warm".to_owned(),
            Uuid::new_v4(),
            candidate.produced_at_unix(),
            candidate.clone(),
        )
        .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    ensure_published(root.path(), &promoted)
        .unwrap_or_else(|error| unreachable!("publish: {error}"));
    let current = load(root.path(), &profile())
        .unwrap_or_else(|error| unreachable!("load: {error}"))
        .unwrap_or_else(|| unreachable!("pointer"));
    assert_eq!(current.current().volume_key(), candidate.volume_key());
    assert_eq!(current.superseded().len(), 1);
    assert_eq!(current.superseded()[0].volume_key(), booted);
    assert_eq!(
        current.superseded()[0].generation(),
        1,
        "the outgoing generation keeps its own number"
    );
}

/// The record reverts on a rolled-back promotion, and the restore has to find
/// the surviving generation by number rather than by position.
#[test]
fn a_rolled_back_promotion_restores_the_generation_the_record_names() {
    let mut published = document(1);
    for generation in 2..=3 {
        let (_, pointer) = capture(generation);
        published = published
            .repointed(
                "flanforge-warm".to_owned(),
                Uuid::new_v4(),
                pointer.produced_at_unix(),
                pointer,
            )
            .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    }
    let restored = published
        .restored(2)
        .unwrap_or_else(|error| unreachable!("restore: {error}"));
    assert_eq!(restored.generation(), 2);
    assert_eq!(restored.current().volume_key(), capture(2).1.volume_key());
    // Generation 3 stays retained rather than being deleted by the restore.
    assert!(
        restored
            .superseded()
            .iter()
            .any(|entry| entry.generation() == 3)
    );
}

/// The retirement age floor is a property of the candidate, not the document,
/// which is why every pointer carries its own timestamp.
#[test]
fn each_superseded_generation_is_aged_on_its_own_timestamp() {
    let published = document(1)
        .repointed(
            "flanforge-warm".to_owned(),
            Uuid::new_v4(),
            capture(2).1.produced_at_unix(),
            capture(2).1,
        )
        .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    let outgoing = &published.superseded()[0];
    let now = outgoing.produced_at_unix() + WARM_RETIREMENT_AGE_SECONDS;
    assert!(is_old_enough(outgoing, now), "exactly at the floor");
    assert!(
        !is_old_enough(published.current(), now),
        "the newer generation is younger than the floor"
    );
    assert!(!is_old_enough(outgoing, now - 1), "one second under");
}

/// Nothing without a capture time can be aged, so a cold base pointer that
/// somehow reached a document is never a deletion candidate.
#[test]
fn a_pointer_with_no_capture_time_is_never_old_enough() {
    let undated = VolumePointer::new(
        "flanforge".to_owned(),
        "base-import-aabbcc.qcow2".to_owned(),
        "/pool/base-import-aabbcc.qcow2".to_owned(),
        40 * GIB,
        0,
        0,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(!is_old_enough(&undated, u64::MAX));
}

/// The live generation is offered for deletion only when nothing can still
/// want it: no profile declares its name, no record claims the profile, and
/// every superseded entry has already been collected.
#[test]
fn only_an_undeclared_unclaimed_profile_offers_its_live_generation() {
    let live = document(1);
    let superseded = live
        .repointed(
            "flanforge-warm".to_owned(),
            Uuid::new_v4(),
            capture(2).1.produced_at_unix(),
            capture(2).1,
        )
        .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    let declared = BTreeMap::from([(
        profile(),
        VmName::new("flanforge-warm").unwrap_or_else(|error| unreachable!("fixture: {error}")),
    )]);
    let claimed = BTreeSet::from([profile()]);
    let empty_names = BTreeMap::new();
    let empty_claims = BTreeSet::new();
    let sweep = |declared: &BTreeMap<ProfileName, VmName>, claimed: &BTreeSet<ProfileName>| {
        candidates(
            &superseded,
            &ImageSweep {
                is_dry_run: false,
                declared,
                claimed,
            },
            &profile(),
        )
    };
    // A declared profile offers its superseded entries and never `current`.
    let offered = sweep(&declared, &empty_claims);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].generation(), 1);
    // Undeclared and unclaimed, but a superseded entry is still outstanding.
    assert_eq!(sweep(&empty_names, &empty_claims).len(), 1);

    let alone = |declared, claimed| {
        candidates(
            &live,
            &ImageSweep {
                is_dry_run: false,
                declared,
                claimed,
            },
            &profile(),
        )
    };
    // Nothing superseded, nothing declaring, nothing claiming: the live
    // generation is the candidate.
    let offered = alone(&empty_names, &empty_claims);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].volume_key(), live.current().volume_key());
    // A record still claims the profile, so the live generation is untouchable
    // even though configuration no longer names it.
    assert!(alone(&empty_names, &claimed).is_empty());
    assert!(alone(&declared, &empty_claims).is_empty());
}

#[test]
fn generalization_names_every_identity_path_it_claims_to_reset() {
    let script = generalization_scripts();
    let paths = identity_paths();
    assert_eq!(
        paths,
        vec![
            "/var/lib/cloud",
            "/var/lib/dbus/machine-id",
            "/var/lib/systemd/random-seed",
            "/var/lib/dhcp",
            "/etc/ssh",
            "ssh_host_*",
            "/var/lib/NetworkManager",
            "*.lease",
            "/etc/machine-id",
        ]
    );
    for path in paths {
        assert!(script.contains(path), "missing {path}");
    }
    assert!(script.contains("exit 12"), "path verification");
    assert!(script.contains("exit 14"), "machine-id verification");
    assert!(
        script.contains("install -m 0444 /dev/null '/etc/machine-id'"),
        "machine-id is replaced with a new regular empty file"
    );
    assert!(script.contains("sudo -n"), "root paths need sudo");
    assert!(!script.contains('\0'));
    assert!(
        std::process::Command::new("/bin/sh")
            .args(["-n", "-c", script.as_str()])
            .status()
            .is_ok_and(|status| status.success()),
        "{script}"
    );
}

#[test]
fn generalization_uses_find_patterns_without_shell_globs() {
    let script = generalization_scripts();
    for pattern in ["ssh_host_*", "*.lease"] {
        assert!(
            script.contains(&format!("-name '{pattern}'")),
            "{pattern}: {script}"
        );
    }
    assert!(!script.contains("rm -rf /etc/ssh/ssh_host_*"), "{script}");
    assert!(
        !script.contains("rm -rf /var/lib/NetworkManager/*.lease"),
        "{script}"
    );
    assert_eq!(script.matches("\\( -type f -o -type l \\)").count(), 4);
}

#[test]
fn generalization_fails_closed_when_identity_removal_or_verification_fails() {
    let script = generalization_scripts();
    assert!(
        script.contains("/bin/rm -rf \"$absolute\" >/dev/null 2>&1 || exit 12"),
        "{script}"
    );
    assert_eq!(
        script.matches("-delete >/dev/null 2>&1 || exit 12").count(),
        2
    );
    assert_eq!(
        script
            .matches("-print -quit 2>/dev/null) || exit 12")
            .count(),
        2
    );
    assert!(!script.contains("|| true"), "{script}");
}

#[test]
fn a_failed_privileged_find_cannot_pass_identity_verification() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let root = directory.path().join("identity-root");
    std::fs::create_dir(&root).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let _sudo =
        flanforge_test_support::executable(directory.path(), "sudo", "#!/bin/sh\nexit 23\n");
    let path = format!("{}:/usr/bin:/bin", directory.path().to_string_lossy());
    let script = find_verify_command_for_test(&root.to_string_lossy(), "*.lease");
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .env("PATH", path)
        .status()
        .unwrap_or_else(|error| unreachable!("shell: {error}"));
    assert_eq!(status.code(), Some(12), "{script}");
}

#[test]
fn generalization_rejects_unsafe_types_before_mutation() {
    let script = generalization_scripts();
    for path in ["/etc/ssh", "/var/lib/NetworkManager", "/etc/machine-id"] {
        let guard = format!("if [ -L '{path}' ]; then exit 15; fi");
        assert_eq!(script.matches(&guard).count(), 2, "{path}: {script}");
    }
    let first_mutation = script
        .find("sudo -n /bin/rm")
        .unwrap_or_else(|| unreachable!("mutation: {script}"));
    for path in ["/etc/ssh", "/var/lib/NetworkManager", "/etc/machine-id"] {
        assert!(
            script
                .find(&format!("if [ -L '{path}' ]"))
                .is_some_and(|guard| guard < first_mutation),
            "{path}: {script}"
        );
    }
    assert!(!script.contains("truncate"), "{script}");
}

#[test]
fn generalization_does_not_strip_workflow_owned_state() {
    let script = generalization_scripts();
    for preserved in [
        REGENERATION_SENTINEL,
        ".gitconfig",
        ".git-credentials",
        ".ssh/known_hosts",
        ".bash_history",
        ".runner",
        "_work",
        "/tmp/flanforged",
        "/var/lib/tailscale",
        "/var/cache/tailscale",
        "/var/lib/containers",
        "/var/log/journal",
        "logout",
    ] {
        assert!(!script.contains(preserved), "{preserved}: {script}");
    }
}

#[test]
fn libvirt_uses_the_shared_read_only_marker_gate() {
    let script = retention_marker_script();
    assert!(script.contains(REGENERATION_SENTINEL));
    for mutation in ["rm ", "mv ", "cp ", "truncate", "logout", "*", "?"] {
        assert!(!script.contains(mutation), "{mutation}: {script}");
    }
}

/// The volume name is derived from the capture id, so recovery can reconstruct
/// it from the checkpoint owner alone and delete by name when no key landed.
#[test]
fn a_warm_checkpoint_names_its_volume_without_a_recorded_key() {
    let capture_id = Uuid::from_u128(42);
    let checkpoint = VolumeCheckpoint::warm(
        Uuid::from_u128(7),
        "flanforge".to_owned(),
        profile().to_string(),
        capture_id,
    )
    .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    assert!(
        checkpoint
            .ensure_warm(Uuid::from_u128(7), "flanforge", "ci", capture_id)
            .is_ok()
    );
    assert!(
        checkpoint
            .ensure_warm(Uuid::from_u128(7), "flanforge", "other", capture_id)
            .is_err()
    );
    assert_eq!(
        VolumeCheckpoint::warm_volume_name(capture_id),
        format!("warm-{capture_id}.qcow2")
    );

    // A checkpoint may not name a volume the capture id does not derive.
    let mut tampered = checkpoint;
    assert!(
        tampered
            .record(
                CheckpointVolumeRole::Warm,
                "warm-other.qcow2".to_owned(),
                "/pool/warm-other.qcow2".to_owned(),
            )
            .is_err()
    );
}

/// The parser and the writer of a capture filename must agree: a drift between
/// them turns recovery into a silent no-op and leaks a full-size volume on
/// every interrupted capture.
#[test]
fn a_capture_filename_round_trips_through_its_parser() {
    let root = state_dir();
    let capture_id = Uuid::from_u128(9);
    let path = crate::checkpoint::warm_path(root.path(), capture_id);
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_else(|| unreachable!("file name"));
    assert_eq!(warm_capture_id(file_name), Some(capture_id));
    assert_eq!(
        warm_capture_id("capture-not-a-uuid.volume-checkpoint.json"),
        None
    );
}

/// Recovery is the only collector for a capture the daemon died inside, so it
/// has to delete exactly the volumes no pointer names and nothing else.
#[tokio::test]
async fn recovery_collects_an_interrupted_capture_and_spares_a_published_one() {
    let helper = FakeHelper::new();
    let root = helper.state_dir();
    let published = document(1);
    ensure_published(root, &published).unwrap_or_else(|error| unreachable!("publish: {error}"));

    // A checkpoint whose volume the live pointer names: publication completed,
    // so only the checkpoint goes.
    let (published_capture, _) = capture(1);
    write_checkpoint(root, published_capture, Boundary::Verified);
    helper
        .worker()
        .ensure_warm_recovered()
        .await
        .unwrap_or_else(|error| unreachable!("recovery: {error}"));
    assert!(
        helper.requests().is_empty(),
        "a published generation is never deleted"
    );
    assert!(!crate::checkpoint::warm_path(root, published_capture).exists());

    // A checkpoint no pointer names is an interrupted capture: it is deleted by
    // both identities the checkpoint recorded.
    let orphan = Uuid::from_u128(0xbeef);
    write_checkpoint(root, orphan, Boundary::Captured);
    helper
        .worker()
        .ensure_warm_recovered()
        .await
        .unwrap_or_else(|error| unreachable!("recovery: {error}"));
    let requests = helper.requests();
    assert_eq!(requests.len(), 1, "one deletion for one orphan");
    let name = VolumeCheckpoint::warm_volume_name(orphan);
    assert!(requests[0].contains("\"operation\":\"delete_volume\""));
    assert!(requests[0].contains(&format!("\"name\":\"{name}\"")));
    assert!(
        requests[0].contains(&format!("\"key\":\"/pool/{name}\"")),
        "the recorded key is deleted too: {}",
        requests[0]
    );
    assert!(!crate::checkpoint::warm_path(root, orphan).exists());
    assert_eq!(
        load(root, &profile()).unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(published),
        "the live pointer is untouched"
    );
}

/// An unreadable checkpoint names a volume that may well be live, so recovery
/// refuses rather than guessing: nothing is deleted and the file stays for an
/// operator to look at.
#[tokio::test]
async fn recovery_leaves_an_unreadable_checkpoint_alone() {
    let helper = FakeHelper::new();
    let root = helper.state_dir();
    let capture_id = Uuid::from_u128(0xfeed);
    let path = crate::checkpoint::warm_path(root, capture_id);
    write_private(&path, b"{ not a checkpoint");

    assert!(helper.worker().ensure_warm_recovered().await.is_err());
    assert!(helper.requests().is_empty(), "nothing was deleted");
    assert!(path.exists());
}

/// The libvirt half of the backend selection contract, against the real
/// implementation: every disagreement between the record and the pointer fails
/// closed to a cold boot, and only an agreeing pair reaches `Ready`. A profile
/// smaller than the image no longer participates — the guest is sized at the
/// image, so availability does not depend on `storage_mb` at all.
#[tokio::test]
async fn warm_availability_is_answered_from_the_pointer_and_fails_closed() {
    let helper = FakeHelper::new();
    let root = helper.state_dir();
    let worker = helper.worker();

    // No pointer at all.
    assert_eq!(
        worker
            .warm_pointer_availability(&warm_record(1, "flanforge-warm"))
            .await
            .unwrap_or_else(|error| unreachable!("availability: {error}")),
        WarmAvailability::Absent
    );

    ensure_published(root, &document(2)).unwrap_or_else(|error| unreachable!("publish: {error}"));
    for (record, expected, reason) in [
        (
            warm_record(1, "flanforge-warm"),
            WarmAvailability::Absent,
            "a stale generation",
        ),
        (
            warm_record(3, "flanforge-warm"),
            WarmAvailability::Absent,
            "a generation ahead of the pointer",
        ),
        (
            warm_record(2, "flanforge-other"),
            WarmAvailability::Absent,
            "another logical name",
        ),
    ] {
        assert_eq!(
            worker
                .warm_pointer_availability(&record)
                .await
                .unwrap_or_else(|error| unreachable!("availability: {error}")),
            expected,
            "{reason}"
        );
    }
    assert!(
        helper.requests().is_empty(),
        "a disagreement is answered without asking the hypervisor"
    );

    // Agreeing record and pointer, and a volume that re-verifies.
    assert_eq!(
        worker
            .warm_pointer_availability(&warm_record(2, "flanforge-warm"))
            .await
            .unwrap_or_else(|error| unreachable!("availability: {error}")),
        WarmAvailability::Ready
    );
    let requests = helper.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("\"operation\":\"check_source\""));
}

/// No image is name-addressed on this backend, so nothing can ever sit under a
/// declared name that the daemon never recorded.
#[tokio::test]
async fn a_pointer_addressed_backend_never_reports_an_unclaimed_image() {
    let helper = FakeHelper::new();
    let name =
        VmName::new("flanforge-warm").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(
        !helper
            .worker()
            .is_unclaimed_image(&name, &[])
            .await
            .unwrap_or_else(|error| unreachable!("unclaimed: {error}"))
    );
    assert!(helper.requests().is_empty());
}

fn warm_record(generation: u64, warm_template: &str) -> WarmImageRecord {
    WarmImageRecord {
        profile: profile(),
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

/// A helper process that records every request it is handed and answers
/// `unit`, so a test can assert what a path asked the hypervisor for without
/// a hypervisor.
struct FakeHelper {
    directory: tempfile::TempDir,
    helper: PathBuf,
}

impl FakeHelper {
    fn new() -> Self {
        let directory = state_dir();
        let log = directory.path().join("requests.log");
        let helper = flanforge_test_support::executable(
            directory.path(),
            "helper",
            &format!(
                "cat >> '{log}'\necho >> '{log}'\nprintf '{{\"result\":\"unit\"}}'\n",
                log = log.display()
            ),
        );
        Self { directory, helper }
    }

    fn state_dir(&self) -> &Path {
        self.directory.path()
    }

    fn worker(&self) -> LibvirtWorker {
        LibvirtWorker::with_helper(self.directory.path(), self.helper.clone())
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
}

#[derive(Clone, Copy, Debug)]
enum Boundary {
    Staged,
    Captured,
    Verified,
}

/// The checkpoint the helper writes before it creates anything, optionally
/// carrying the key it records afterwards.
fn write_checkpoint(state_dir: &Path, capture_id: Uuid, boundary: Boundary) {
    let mut checkpoint = VolumeCheckpoint::warm(
        Uuid::from_u128(7),
        "flanforge".to_owned(),
        profile().to_string(),
        capture_id,
    )
    .unwrap_or_else(|error| unreachable!("checkpoint: {error}"));
    let name = VolumeCheckpoint::warm_volume_name(capture_id);
    if !matches!(boundary, Boundary::Staged) {
        checkpoint
            .record(
                CheckpointVolumeRole::Warm,
                name.clone(),
                format!("/pool/{name}"),
            )
            .unwrap_or_else(|error| unreachable!("record: {error}"));
    }
    let bytes = checkpoint
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    write_private(&crate::checkpoint::warm_path(state_dir, capture_id), &bytes);
}

fn write_private(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        super::ensure_private_directory(parent)
            .unwrap_or_else(|error| unreachable!("directory: {error}"));
    }
    std::fs::write(path, bytes).unwrap_or_else(|error| unreachable!("write: {error}"));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|error| unreachable!("permissions: {error}"));
}
