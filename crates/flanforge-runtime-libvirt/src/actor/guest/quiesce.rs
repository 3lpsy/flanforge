use std::{thread, time::Instant};

use flanforge_libvirt_wire::OwnershipManifest;
use virt::{connect::Connect, domain::Domain};

use crate::{RuntimeError, manifest::ensure_recovery_artifacts};

use super::{exact_domain, free_domain};

/// Refuses unless this allocation's domain exists and is not running, so a
/// capture can never read a disk a guest is still writing.
pub(in crate::actor) fn ensure_inactive(
    connection: &Connect,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    let mut domain = exact_domain(connection, manifest)?;
    let active = domain
        .is_active()
        .map_err(|error| RuntimeError::libvirt("domain state", error));
    free_domain(&mut domain)?;
    if active? {
        return Err(RuntimeError::ownership(
            "refusing to capture a running guest",
        ));
    }
    Ok(())
}

/// Ordered shutdown with no undefine and no volume deletion.
///
/// Deliberately not `stop`: that escalates to a forced power-off, which would
/// capture the overlay mid-write. Identity generalization is verified through
/// the live filesystem, so an unflushed reset could be absent from the captured
/// bytes while the check reported success. A guest that will not shut down
/// cleanly fails the Stop phase instead, exactly as Tart's does.
pub(in crate::actor) fn quiesce(
    connection: &Connect,
    manifest: &OwnershipManifest,
    deadline: Instant,
) -> Result<(), RuntimeError> {
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    ensure_recovery_artifacts(manifest)?;
    let mut domain = exact_domain(connection, manifest)?;
    let result = shutdown_gracefully(&domain, deadline);
    free_domain(&mut domain)?;
    result
}

fn shutdown_gracefully(domain: &Domain, deadline: Instant) -> Result<(), RuntimeError> {
    if !domain
        .is_active()
        .map_err(|error| RuntimeError::libvirt("domain state", error))?
    {
        return Ok(());
    }
    domain
        .shutdown()
        .map_err(|error| RuntimeError::libvirt("domain shutdown", error))?;
    while Instant::now() < deadline {
        if !domain
            .is_active()
            .map_err(|error| RuntimeError::libvirt("domain state", error))?
        {
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(RuntimeError::Deadline)
}
