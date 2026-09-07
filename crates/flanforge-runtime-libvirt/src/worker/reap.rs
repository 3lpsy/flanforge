use std::{path::Path, time::Duration};

use flanforge_core::{Allocation, VmName};
use flanforge_libvirt_wire::OwnershipManifest;
use flanforge_manager::{CleanupBudget, WorkerError};
use uuid::Uuid;

use crate::manifest::{ensure_reapable, find_by_domain, load, path, record_cleanup};

use super::{LibvirtWorker, cleanup::remove_authority_files};

impl LibvirtWorker {
    /// Destroys everything one ownership manifest claims, then retires that
    /// manifest's durable authority.
    ///
    /// The reaper and hot eviction share this: they differ only in the evidence
    /// that resolved the manifest, never in what deletion means. Ownership is
    /// re-checked here rather than trusted from the caller.
    pub(super) async fn ensure_reaped(
        &self,
        allocation_id: Uuid,
        name: &VmName,
        budget: CleanupBudget,
        registration: Option<&Allocation>,
    ) -> Result<(), WorkerError> {
        let manifest = load_owned(&self.state_dir, allocation_id).await?;
        ensure_reapable(&manifest, allocation_id, name, &self.state_dir)?;
        let timeout = budget
            .at_least(Duration::from_secs(
                flanforge_core::LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS,
            ))
            .timeout();
        self.actor.cleanup(manifest.clone(), timeout).await?;
        // The registration outlives the guest otherwise, and nothing later
        // rediscovers it.
        if let Some(allocation) = registration
            && let Err(error) = self.registration.delete_runner(allocation).await
        {
            tracing::warn!(%error, "reaped guest left its Forgejo registration");
        }
        let state_dir = self.state_dir.clone();
        tokio::task::spawn_blocking(move || record_cleanup(&state_dir, &manifest))
            .await
            .map_err(|_| WorkerError::new("cleanup tombstone task failed"))??;
        remove_authority_files(&self.state_dir, allocation_id).await
    }
}

/// The allocation whose durable manifest still claims this domain. That
/// manifest is ownership evidence a name cannot forge, and it is the only way
/// to reach a machine whose own allocation record has aged out.
pub(super) async fn find_allocation(state_dir: &Path, name: &VmName) -> Result<Uuid, WorkerError> {
    let state_dir = state_dir.to_path_buf();
    let domain = name.as_str().to_owned();
    Ok(
        tokio::task::spawn_blocking(move || find_by_domain(&state_dir, &domain))
            .await
            .map_err(|_| WorkerError::new("ownership lookup task failed"))??,
    )
}

/// The ownership manifest one allocation wrote, which is the only authority
/// over the domain and volumes it claims.
pub(super) async fn load_owned(
    state_dir: &Path,
    allocation_id: Uuid,
) -> Result<OwnershipManifest, WorkerError> {
    let manifest_path = path(state_dir, allocation_id);
    tokio::task::spawn_blocking(move || load(&manifest_path))
        .await
        .map_err(|_| WorkerError::new("ownership load task failed"))?
        .map_err(Into::into)
}
