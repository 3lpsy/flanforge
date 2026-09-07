use std::collections::BTreeSet;

use virt::{
    connect::Connect, error::ErrorNumber, storage_pool::StoragePool, storage_vol::StorageVol,
};

use crate::{RuntimeError, file::normalize_path};

use super::super::super::handles::{free_pool, free_volume};

/// Everything that currently names another volume as its backing file, or is
/// attached as a disk.
///
/// Absence of evidence is never proof, so every reading that cannot be taken
/// is an error rather than an empty answer.
#[derive(Debug, Default)]
pub(in crate::actor::storage::warm) struct BackingInventory {
    paths: BTreeSet<String>,
    keys: BTreeSet<String>,
}

impl BackingInventory {
    pub(in crate::actor::storage::warm) fn is_referenced(&self, key: &str, path: &str) -> bool {
        self.keys.contains(key) || self.paths.contains(&normalize_path(path))
    }

    pub(super) fn insert(&mut self, connection: &Connect, path: &str) -> Result<(), RuntimeError> {
        self.paths.insert(normalize_path(path));
        // libvirt resolves the path itself, which is what makes a pool
        // directory reached through a symlink compare equal to its volume.
        match StorageVol::lookup_by_path(connection, path) {
            Ok(mut volume) => {
                let key = volume
                    .get_key()
                    .map_err(|error| RuntimeError::libvirt("backing volume key", error));
                free_volume(&mut volume)?;
                self.keys.insert(key?);
                Ok(())
            }
            Err(error) if error.code() == ErrorNumber::NoStorageVolume => Ok(()),
            Err(error) => Err(RuntimeError::libvirt("backing volume lookup", error)),
        }
    }

    /// Records a pool-addressed disk by both of its identities, and reports the
    /// path it resolved to so the caller can walk its backing chain.
    pub(super) fn insert_volume(
        &mut self,
        connection: &Connect,
        pool_name: &str,
        volume_name: &str,
    ) -> Result<Option<String>, RuntimeError> {
        let mut pool = match StoragePool::lookup_by_name(connection, pool_name) {
            Ok(pool) => pool,
            Err(error) if error.code() == ErrorNumber::NoStoragePool => return Ok(None),
            Err(error) => return Err(RuntimeError::libvirt("disk pool lookup", error)),
        };
        let mut volume = match StorageVol::lookup_by_name(&pool, volume_name) {
            Ok(volume) => volume,
            Err(error) if error.code() == ErrorNumber::NoStorageVolume => {
                free_pool(&mut pool)?;
                return Ok(None);
            }
            Err(error) => {
                let _ = free_pool(&mut pool);
                return Err(RuntimeError::libvirt("disk volume lookup", error));
            }
        };
        let identity = volume
            .get_key()
            .and_then(|key| volume.get_path().map(|path| (key, path)))
            .map_err(|error| RuntimeError::libvirt("disk volume identity", error));
        free_volume(&mut volume)?;
        free_pool(&mut pool)?;
        let (key, path) = identity?;
        self.keys.insert(key);
        self.paths.insert(normalize_path(&path));
        Ok(Some(path))
    }
}
