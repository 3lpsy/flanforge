use flanforge_libvirt_wire::VolumePointer;
use virt::{
    connect::Connect, error::ErrorNumber, storage_pool::StoragePool, storage_vol::StorageVol,
};

use crate::{RuntimeError, actor::message::ActorConfig};

use super::super::handles::{free_pool, free_volume};
use super::inventory::{BackingInventory, ensure_domains_walked, referenced_backing_paths};

/// Proves and deletes in one call.
///
/// That is the load-bearing safety argument: `create_guest_resources` is
/// itself a mutating request, and every mutation serializes behind one actor
/// mutex, so within this daemon no overlay can be created between proving a
/// generation unreferenced and deleting it. Splitting proof from delete
/// silently breaks that. The enumeration runs once for the whole batch, since
/// the same argument holds for a batch as for a single candidate.
pub(in crate::actor) fn retire(
    connection: &Connect,
    config: &ActorConfig,
    candidates: &[VolumePointer],
    protected: &[VolumePointer],
) -> Result<Vec<String>, RuntimeError> {
    for candidate in candidates {
        if candidate.pool() != config.pool {
            return Err(RuntimeError::ownership(
                "retirement candidate belongs to another storage pool",
            ));
        }
        if protected
            .iter()
            .any(|pointer| pointer.is_same_volume(candidate))
        {
            return Err(RuntimeError::ownership(
                "retirement candidate is a protected generation",
            ));
        }
    }
    let mut inventory = referenced_backing_paths(connection, &config.pool)?;
    // Defence in depth for a domain whose disk sits outside the pool.
    ensure_domains_walked(connection, &mut inventory)?;
    let mut deleted = Vec::new();
    for candidate in candidates {
        if delete_if_unreferenced(connection, config, candidate, &inventory)? {
            deleted.push(candidate.volume_key().to_owned());
        }
    }
    Ok(deleted)
}

fn delete_if_unreferenced(
    connection: &Connect,
    config: &ActorConfig,
    candidate: &VolumePointer,
    inventory: &BackingInventory,
) -> Result<bool, RuntimeError> {
    let mut pool = StoragePool::lookup_by_name(connection, &config.pool)
        .map_err(|error| RuntimeError::libvirt("retirement pool lookup", error))?;
    let mut volume = match StorageVol::lookup_by_name(&pool, candidate.volume_name()) {
        Ok(volume) => volume,
        // A generation whose volume is already gone retires by bookkeeping.
        Err(error) if error.code() == ErrorNumber::NoStorageVolume => {
            free_pool(&mut pool)?;
            return Ok(true);
        }
        Err(error) => {
            let _ = free_pool(&mut pool);
            return Err(RuntimeError::libvirt("retirement volume lookup", error));
        }
    };
    let result = (|| {
        let key = volume
            .get_key()
            .map_err(|error| RuntimeError::libvirt("retirement volume key", error))?;
        let path = volume
            .get_path()
            .map_err(|error| RuntimeError::libvirt("retirement volume path", error))?;
        if key != candidate.volume_key() {
            return Err(RuntimeError::ownership(
                "retirement candidate disagrees with the configured-pool volume",
            ));
        }
        if inventory.is_referenced(&key, &path) {
            return Ok(false);
        }
        volume
            .delete(0)
            .map_err(|error| RuntimeError::libvirt("retirement volume deletion", error))?;
        Ok(true)
    })();
    free_volume(&mut volume)?;
    free_pool(&mut pool)?;
    result
}
