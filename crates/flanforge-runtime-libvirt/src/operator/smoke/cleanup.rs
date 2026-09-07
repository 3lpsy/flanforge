use std::time::Duration;

use flanforge_core::VmName;

use crate::{
    RuntimeError,
    actor::LibvirtActor,
    checkpoint::{allocation_path as checkpoint_path, load as load_checkpoint, merge_allocation},
    manifest::{ServiceInstance, ensure_matches},
};

use super::state;

pub(super) async fn recover_and_cleanup(
    state_dir: &std::path::Path,
    actor: &LibvirtActor,
    instance: &ServiceInstance,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let Some(mut manifest) = state::load_pending(state_dir).await? else {
        return Ok(());
    };
    let vm_name = VmName::new(manifest.domain_name()).map_err(RuntimeError::ownership)?;
    ensure_matches(
        &manifest,
        manifest.allocation_id(),
        &vm_name,
        instance,
        state_dir,
    )?;
    let checkpoint_file = checkpoint_path(state_dir, manifest.allocation_id());
    let checkpoint = tokio::task::spawn_blocking(move || load_checkpoint(&checkpoint_file))
        .await
        .map_err(|_| RuntimeError::manifest("smoke checkpoint task failed"))??;
    if let Some(checkpoint) = checkpoint {
        merge_allocation(&checkpoint, &mut manifest, instance.id(), actor.pool())?;
        state::persist(state_dir, &manifest).await?;
    }
    actor.cleanup(manifest.clone(), timeout).await?;
    state::remove(state_dir, &manifest).await
}

pub(super) fn finish(
    operation: Result<super::super::SmokeOutcome, RuntimeError>,
    cleanup: Result<(), RuntimeError>,
) -> Result<super::super::SmokeOutcome, RuntimeError> {
    match (operation, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(operation), Err(cleanup)) => Err(RuntimeError::cleanup(operation, cleanup)),
    }
}
