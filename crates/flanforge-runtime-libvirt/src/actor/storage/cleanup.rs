use flanforge_libvirt_wire::Artifact;
use virt::{
    connect::Connect, error::ErrorNumber, storage_pool::StoragePool, storage_vol::StorageVol,
};

use crate::RuntimeError;

use super::handles::{free_pool, free_volume};

/// Deletes one owned volume from the configured pool.
///
/// A `None` key is the crash window between creating a volume and recording
/// its key: the name still carries this allocation's own artifact id, so it
/// cannot name another tool's volume, and leaving it would strand a full-size
/// image no later sweep can see.
pub(in crate::actor) fn delete_exact(
    connection: &Connect,
    pool_name: &str,
    key: Option<&str>,
    expected_name: &str,
) -> Result<(), RuntimeError> {
    let mut pool = StoragePool::lookup_by_name(connection, pool_name)
        .map_err(|error| RuntimeError::libvirt("owned pool lookup", error))?;
    let mut volume = match StorageVol::lookup_by_name(&pool, expected_name) {
        Ok(volume) => volume,
        Err(error) if error.code() == ErrorNumber::NoStorageVolume => {
            free_pool(&mut pool)?;
            return Ok(());
        }
        Err(error) => {
            let _ = free_pool(&mut pool);
            return Err(RuntimeError::libvirt("owned volume lookup", error));
        }
    };
    let live_name = volume
        .get_name()
        .map_err(|error| RuntimeError::libvirt("owned volume name", error))?;
    let live_key = volume
        .get_key()
        .map_err(|error| RuntimeError::libvirt("owned volume key", error))?;
    if let Err(error) = ensure_identity(expected_name, key, &live_name, &live_key) {
        let _ = free_volume(&mut volume);
        let _ = free_pool(&mut pool);
        return Err(error);
    }
    volume
        .delete(0)
        .map_err(|error| RuntimeError::libvirt("volume deletion", error))?;
    free_volume(&mut volume)?;
    free_pool(&mut pool)
}

pub(in crate::actor) fn delete_artifact(
    connection: &Connect,
    pool_name: &str,
    artifact: &Artifact,
) -> Result<(), RuntimeError> {
    delete_exact(connection, pool_name, artifact.key(), artifact.name())
}

pub(in crate::actor) fn ensure_volume(
    connection: &Connect,
    pool_name: &str,
    key: &str,
    name: &str,
) -> Result<(), RuntimeError> {
    let mut pool = StoragePool::lookup_by_name(connection, pool_name)
        .map_err(|error| RuntimeError::libvirt("owned pool lookup", error))?;
    let mut volume = StorageVol::lookup_by_name(&pool, name)
        .map_err(|error| RuntimeError::libvirt("owned volume lookup", error))?;
    let live_name = volume
        .get_name()
        .map_err(|error| RuntimeError::libvirt("owned volume name", error))?;
    let live_key = volume
        .get_key()
        .map_err(|error| RuntimeError::libvirt("owned volume key", error))?;
    free_volume(&mut volume)?;
    free_pool(&mut pool)?;
    ensure_identity(name, Some(key), &live_name, &live_key)
}

fn ensure_identity(
    expected_name: &str,
    expected_key: Option<&str>,
    live_name: &str,
    live_key: &str,
) -> Result<(), RuntimeError> {
    if live_name != expected_name || expected_key.is_some_and(|key| key != live_key) {
        Err(RuntimeError::ownership(
            "configured-pool volume disagrees with the ownership manifest",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
