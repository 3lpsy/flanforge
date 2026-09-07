use std::{thread, time::Instant};

use flanforge_libvirt_wire::{Artifact, OwnershipManifest};
use virt::{connect::Connect, domain::Domain, error::ErrorNumber};

use crate::{RuntimeError, manifest::ensure_recovery_artifacts};

use super::super::{message::ActorConfig, storage};
use super::{ensure_domain, free_domain};

pub(in crate::actor) fn cleanup(
    connection: &Connect,
    config: &ActorConfig,
    manifest: &OwnershipManifest,
    deadline: Instant,
) -> Result<(), RuntimeError> {
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    ensure_recovery_artifacts(manifest)?;
    match Domain::lookup_by_uuid(connection, manifest.domain_uuid()) {
        Ok(mut domain) => {
            ensure_domain(&domain, manifest)?;
            stop(&domain, deadline)?;
            domain
                .undefine()
                .map_err(|error| RuntimeError::libvirt("domain undefine", error))?;
            free_domain(&mut domain)?;
        }
        Err(error) if error.code() == ErrorNumber::NoDomain => {}
        Err(error) => return Err(RuntimeError::libvirt("domain lookup", error)),
    }
    cleanup_artifacts(manifest, |artifact| {
        storage::delete_artifact(connection, &config.pool, artifact)
    })
}

pub(super) fn cleanup_artifacts(
    manifest: &OwnershipManifest,
    mut delete: impl FnMut(&Artifact) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    let seed = delete_if_owned(manifest.seed(), &mut delete);
    let overlay = delete_if_owned(manifest.overlay(), &mut delete);
    match (seed, overlay) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(seed), Err(overlay)) => Err(RuntimeError::libvirt(
            "allocation volume cleanup",
            format!("seed: {seed}; overlay: {overlay}"),
        )),
    }
}

/// A volume created before its key was recorded still has to go: reporting
/// success for it stranded a full-size image nothing could later find.
/// Deletion tolerates an absent volume, so a never-created artifact is a no-op.
fn delete_if_owned(
    artifact: &Artifact,
    delete: &mut impl FnMut(&Artifact) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    delete(artifact)
}

fn stop(domain: &Domain, deadline: Instant) -> Result<(), RuntimeError> {
    if !domain
        .is_active()
        .map_err(|error| RuntimeError::libvirt("domain state", error))?
    {
        return Ok(());
    }
    let _ = domain.shutdown();
    while Instant::now() < deadline {
        if !domain
            .is_active()
            .map_err(|error| RuntimeError::libvirt("domain state", error))?
        {
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(200));
    }
    let Err(error) = domain.destroy() else {
        return Ok(());
    };
    // A guest that stopped on its own between the last poll and the destroy is
    // stopped, and must not withhold the undefine and volume deletes that
    // follow. Only a domain still running does that.
    match domain.is_active() {
        Ok(false) => Ok(()),
        Err(state) if state.code() == ErrorNumber::NoDomain => Ok(()),
        _ => Err(RuntimeError::libvirt("domain forced stop", error)),
    }
}
