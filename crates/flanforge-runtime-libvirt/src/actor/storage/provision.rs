use flanforge_libvirt_wire::{CheckpointVolumeRole, OwnershipManifest, VolumePointer};
use virt::{connect::Connect, storage_pool::StoragePool, storage_vol::StorageVol};

use crate::{
    RuntimeError,
    actor::message::{ActorConfig, CreateRequest},
    checkpoint::VolumeJournal,
    domain::{overlay_xml, seed_xml},
};

use super::{
    cleanup::delete_exact,
    handles::{free_pool, free_volume},
    import::upload,
};

pub(in crate::actor) fn create_guest_resources(
    connection: &Connect,
    config: &ActorConfig,
    mut request: CreateRequest,
) -> Result<OwnershipManifest, RuntimeError> {
    request
        .manifest
        .ensure_valid()
        .map_err(RuntimeError::manifest)?;
    let immediate_bytes = u64::try_from(request.seed.len())
        .map_err(|_| RuntimeError::manifest("seed size overflows capacity"))?;
    let mut pool = active_pool(connection, config, immediate_bytes)?;
    let backing_path = check_source_pointer(connection, config, &request.source)?;
    // An overlay is never smaller than what it backs onto. The daemon floors
    // the request already; repeating it here keeps a stale one from shrinking
    // a base rather than refusing the allocation over it.
    let capacity = request.storage_bytes.max(request.source.virtual_bytes());
    let mut journal = VolumeJournal::allocation(config, &request.manifest)?;
    let mut overlay = create_volume(
        &pool,
        &overlay_xml(request.manifest.overlay().name(), capacity, &backing_path)?,
        "overlay creation",
    )?;
    let overlay_key = match record_created(
        &mut journal,
        CheckpointVolumeRole::Overlay,
        request.manifest.overlay().name(),
        &overlay,
        "overlay key",
    ) {
        Ok(key) => key,
        Err(error) => {
            let _ = overlay.delete(0);
            let _ = free_volume(&mut overlay);
            let _ = free_pool(&mut pool);
            return Err(error);
        }
    };
    request.manifest.overlay_mut().set_key(overlay_key.clone());
    free_volume(&mut overlay)?;

    let seed_result = create_seed(
        connection,
        &pool,
        request.manifest.seed().name(),
        &request.seed,
        &mut journal,
    );
    let seed_key = match seed_result {
        Ok(key) => key,
        Err(error) => {
            let _ = delete_exact(
                connection,
                &config.pool,
                Some(&overlay_key),
                request.manifest.overlay().name(),
            );
            let _ = free_pool(&mut pool);
            return Err(error);
        }
    };
    request.manifest.seed_mut().set_key(seed_key);
    free_pool(&mut pool)?;
    request
        .manifest
        .ensure_valid()
        .map_err(RuntimeError::manifest)?;
    Ok(request.manifest)
}

pub(super) fn active_pool(
    connection: &Connect,
    config: &ActorConfig,
    immediate_bytes: u64,
) -> Result<StoragePool, RuntimeError> {
    let pool = StoragePool::lookup_by_name(connection, &config.pool)
        .map_err(|error| RuntimeError::libvirt("storage-pool lookup", error))?;
    let info = pool
        .get_info()
        .map_err(|error| RuntimeError::libvirt("storage-pool capacity", error))?;
    let required = config
        .min_storage_free_bytes
        .checked_add(immediate_bytes)
        .ok_or_else(|| RuntimeError::manifest("required storage capacity overflows"))?;
    if !pool
        .is_active()
        .map_err(|error| RuntimeError::libvirt("storage-pool state", error))?
        || info.available < required
    {
        return Err(RuntimeError::capacity(
            "configured storage reserve is unavailable",
        ));
    }
    Ok(pool)
}

/// Resolves one boot source, cold or warm, to the path an overlay backs onto.
///
/// The helper is indifferent to which it is: both are immutable, both are
/// identified by (`volume_name`, `volume_key`), and the declared virtual size
/// is the only capacity claim the boot path ever enforced.
pub(in crate::actor) fn check_source_pointer(
    connection: &Connect,
    config: &ActorConfig,
    pointer: &VolumePointer,
) -> Result<String, RuntimeError> {
    pointer.ensure_valid().map_err(RuntimeError::manifest)?;
    if pointer.pool() != config.pool {
        return Err(RuntimeError::ownership(
            "published base belongs to another storage pool",
        ));
    }
    let mut pool = StoragePool::lookup_by_name(connection, &config.pool)
        .map_err(|error| RuntimeError::libvirt("published base pool lookup", error))?;
    let mut volume = StorageVol::lookup_by_name(&pool, pointer.volume_name())
        .map_err(|error| RuntimeError::libvirt("published base lookup", error))?;
    let key = volume
        .get_key()
        .map_err(|error| RuntimeError::libvirt("published base key", error))?;
    let info = volume
        .get_info()
        .map_err(|error| RuntimeError::libvirt("published base capacity", error))?;
    let path = volume
        .get_path()
        .map_err(|error| RuntimeError::libvirt("published base path", error))?;
    free_volume(&mut volume)?;
    free_pool(&mut pool)?;
    if key != pointer.volume_key() || info.capacity != pointer.virtual_bytes() {
        return Err(RuntimeError::ownership(
            "published base pointer disagrees with the configured-pool volume",
        ));
    }
    Ok(path)
}

fn create_seed(
    connection: &Connect,
    pool: &StoragePool,
    name: &str,
    bytes: &[u8],
    journal: &mut VolumeJournal,
) -> Result<String, RuntimeError> {
    let mut volume = create_volume(pool, &seed_xml(name, bytes.len()), "seed creation")?;
    let result = (|| {
        let key = record_created(
            journal,
            CheckpointVolumeRole::Seed,
            name,
            &volume,
            "seed key",
        )?;
        upload(
            connection,
            &volume,
            &mut std::io::Cursor::new(bytes),
            bytes.len() as u64,
            None,
        )?;
        Ok(key)
    })();
    if result.is_err() {
        let _ = volume.delete(0);
    }
    free_volume(&mut volume)?;
    result
}

fn record_created(
    journal: &mut VolumeJournal,
    role: CheckpointVolumeRole,
    name: &str,
    volume: &StorageVol,
    operation: &'static str,
) -> Result<String, RuntimeError> {
    let key = volume
        .get_key()
        .map_err(|error| RuntimeError::libvirt(operation, error))?;
    journal.record(role, name.to_owned(), key.clone())?;
    Ok(key)
}

pub(super) fn create_volume(
    pool: &StoragePool,
    xml: &str,
    operation: &'static str,
) -> Result<StorageVol, RuntimeError> {
    StorageVol::create_xml(pool, xml, 0)
        .map_err(|error| create_error(operation, error.code(), error))
}

fn create_error(
    operation: &'static str,
    code: virt::error::ErrorNumber,
    error: impl std::fmt::Display,
) -> RuntimeError {
    if code == virt::error::ErrorNumber::StorageVolExist {
        RuntimeError::collision(operation, error)
    } else {
        RuntimeError::libvirt(operation, error)
    }
}

#[cfg(test)]
mod tests;
