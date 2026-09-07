use flanforge_libvirt_wire::{CheckpointVolumeRole, VolumeCheckpoint, VolumePointer};
use virt::{connect::Connect, storage_pool::StoragePool, storage_vol::StorageVol};

use crate::{
    RuntimeError,
    actor::message::{ActorConfig, WarmCaptureRequest},
    checkpoint::VolumeJournal,
    domain::warm_xml,
    manifest::ensure_recovery_artifacts,
};

use super::super::{
    cleanup::ensure_volume,
    handles::{free_pool, free_volume},
    provision::active_pool,
};

/// Flattens this allocation's quiesced overlay into a new standalone volume.
///
/// Purely additive: it creates one volume that nothing yet references and
/// replaces nothing. libvirt performs the copy, so no flanforge process opens
/// a pool path — the hypervisor may be on another host, and the service unit's
/// mount namespace makes the pool directory read-only even when it is not.
pub(in crate::actor) fn capture(
    connection: &Connect,
    config: &ActorConfig,
    request: &WarmCaptureRequest,
) -> Result<VolumePointer, RuntimeError> {
    request
        .manifest
        .ensure_valid()
        .map_err(RuntimeError::manifest)?;
    // The same ownership re-verification cleanup does, so a capture can only
    // ever read this allocation's own overlay.
    ensure_recovery_artifacts(&request.manifest)?;
    crate::actor::guest::ensure_inactive(connection, &request.manifest)?;
    let overlay_key = request
        .manifest
        .overlay()
        .key()
        .ok_or_else(|| RuntimeError::ownership("overlay key is absent"))?;
    ensure_volume(
        connection,
        &config.pool,
        overlay_key,
        request.manifest.overlay().name(),
    )?;
    let mut pool = active_pool(connection, config, request.virtual_bytes)?;
    let mut journal = VolumeJournal::warm(config, &request.profile, request.capture_id)?;
    let volume_name = VolumeCheckpoint::warm_volume_name(request.capture_id);
    let result = create_flattened(
        connection,
        &pool,
        &volume_name,
        request.manifest.overlay().name(),
        request.virtual_bytes,
        &mut journal,
    );
    free_pool(&mut pool)?;
    let key = result?;
    VolumePointer::new(
        config.pool.clone(),
        volume_name,
        key,
        request.virtual_bytes,
        request.generation,
        unix_time(),
    )
    .map_err(RuntimeError::manifest)
}

/// The capture time is taken helper-side, where the volume actually exists.
fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn create_flattened(
    connection: &Connect,
    pool: &StoragePool,
    volume_name: &str,
    overlay_name: &str,
    virtual_bytes: u64,
    journal: &mut VolumeJournal,
) -> Result<String, RuntimeError> {
    let mut overlay = StorageVol::lookup_by_name(pool, overlay_name)
        .map_err(|error| RuntimeError::libvirt("warm source lookup", error))?;
    let created =
        StorageVol::create_xml_from(pool, &warm_xml(volume_name, virtual_bytes), &overlay, 0)
            .map_err(|error| RuntimeError::libvirt("warm capture", error));
    free_volume(&mut overlay)?;
    let mut volume = created?;
    let result = (|| {
        let key = volume
            .get_key()
            .map_err(|error| RuntimeError::libvirt("warm capture key", error))?;
        journal.record(
            CheckpointVolumeRole::Warm,
            volume_name.to_owned(),
            key.clone(),
        )?;
        Ok(key)
    })();
    if result.is_err() {
        let _ = volume.delete(0);
    }
    free_volume(&mut volume)?;
    let _ = connection;
    result
}
