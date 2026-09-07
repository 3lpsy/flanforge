use std::path::PathBuf;

use flanforge_libvirt_wire::{Artifact, OwnershipManifest};
use uuid::Uuid;

use crate::RuntimeError;

use super::cleanup::cleanup_artifacts;

#[test]
fn artifact_cleanup_attempts_both_independent_volumes_before_returning() {
    let artifact_id = Uuid::new_v4();
    let mut manifest = OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "ci-libvirt-test".to_owned(),
        Uuid::new_v4(),
        "02:01:02:03:04:05".to_owned(),
        Artifact::new(format!("root-{artifact_id}.qcow2")),
        Artifact::new(format!("seed-{artifact_id}.img")),
        "flanforge-test".to_owned(),
        PathBuf::from("/var/lib/flanforge/known_hosts"),
        1,
    );
    manifest
        .overlay_mut()
        .set_key("/pool/root.qcow2".to_owned());
    manifest.seed_mut().set_key("/pool/seed.img".to_owned());
    let mut attempted = Vec::new();
    let result = cleanup_artifacts(&manifest, |artifact| {
        attempted.push(artifact.name().to_owned());
        Err(RuntimeError::libvirt("injected deletion", artifact.name()))
    });
    let Err(error) = result else {
        unreachable!("injected deletions must fail")
    };

    assert_eq!(
        attempted,
        [manifest.seed().name(), manifest.overlay().name()]
    );
    assert!(error.to_string().contains("seed:"));
    assert!(error.to_string().contains("overlay:"));
}

/// A volume created before its key reached the journal is still this
/// allocation's, and its name carries this allocation's artifact id. Skipping
/// it stranded a full-size image nothing could later find, so cleanup now
/// attempts both and lets the pool-scoped name check refuse anything else.
#[test]
fn partial_cleanup_attempts_every_named_artifact() {
    let artifact_id = Uuid::new_v4();
    let mut manifest = OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "ci-libvirt-partial".to_owned(),
        Uuid::new_v4(),
        "02:01:02:03:04:06".to_owned(),
        Artifact::new(format!("root-{artifact_id}.qcow2")),
        Artifact::new(format!("seed-{artifact_id}.img")),
        "flanforge-partial".to_owned(),
        PathBuf::from("/var/lib/flanforge/known_hosts"),
        1,
    );
    manifest
        .overlay_mut()
        .set_key("/pool/root-partial.qcow2".to_owned());
    let mut attempted = Vec::new();
    cleanup_artifacts(&manifest, |artifact| {
        attempted.push(artifact.name().to_owned());
        Ok(())
    })
    .unwrap_or_else(|error| unreachable!("cleanup: {error}"));
    assert_eq!(
        attempted,
        [manifest.seed().name(), manifest.overlay().name()]
    );
}
