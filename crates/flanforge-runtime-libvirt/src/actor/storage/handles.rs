use virt::{network::Network, storage_pool::StoragePool, storage_vol::StorageVol};

use crate::RuntimeError;

pub(super) fn free_volume(volume: &mut StorageVol) -> Result<(), RuntimeError> {
    volume
        .free()
        .map_err(|error| RuntimeError::libvirt("volume handle release", error))
}

pub(super) fn free_pool(pool: &mut StoragePool) -> Result<(), RuntimeError> {
    pool.free()
        .map_err(|error| RuntimeError::libvirt("pool handle release", error))
}

pub(super) fn free_network(network: &mut Network) -> Result<(), RuntimeError> {
    network
        .free()
        .map_err(|error| RuntimeError::libvirt("network handle release", error))
}
