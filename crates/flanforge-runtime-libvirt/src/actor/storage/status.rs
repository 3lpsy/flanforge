use roxmltree::Document;
use virt::{connect::Connect, network::Network, storage_pool::StoragePool};

use crate::RuntimeError;

use crate::actor::message::ActorConfig;

use super::handles::{free_network, free_pool};

pub(in crate::actor) fn probe(
    connection: &Connect,
    config: &ActorConfig,
) -> Result<(), RuntimeError> {
    let mut pool = StoragePool::lookup_by_name(connection, &config.pool)
        .map_err(|error| RuntimeError::libvirt("storage-pool lookup", error))?;
    let pool_active = pool
        .is_active()
        .map_err(|error| RuntimeError::libvirt("storage-pool state", error))?;
    let info = pool
        .get_info()
        .map_err(|error| RuntimeError::libvirt("storage-pool capacity", error))?;
    // Config validation is offline and cannot see the pool type, so the one
    // place that can refuses here rather than deep inside a promotion.
    let directory_backed = if config.is_warm_declared {
        is_directory_pool(&pool)?
    } else {
        true
    };
    let mut network = Network::lookup_by_name(connection, &config.network)
        .map_err(|error| RuntimeError::libvirt("network lookup", error))?;
    let network_active = network
        .is_active()
        .map_err(|error| RuntimeError::libvirt("network state", error))?;
    free_network(&mut network)?;
    free_pool(&mut pool)?;
    if !directory_backed {
        return Err(RuntimeError::Configuration {
            message: "warm images require a directory-backed storage pool".to_owned(),
        });
    }
    if !pool_active || !network_active || info.available < config.min_storage_free_bytes {
        return Err(RuntimeError::capacity(
            "pool, network, or storage reserve is unavailable",
        ));
    }
    Ok(())
}

fn is_directory_pool(pool: &StoragePool) -> Result<bool, RuntimeError> {
    let xml = pool
        .get_xml_desc(0)
        .map_err(|error| RuntimeError::libvirt("storage-pool type", error))?;
    is_directory_pool_xml(&xml)
}

/// The retirement proof compares backing paths while ownership compares keys,
/// and only a directory pool makes those the same string. LVM, RBD, and
/// gluster are refused rather than silently unsound.
fn is_directory_pool_xml(xml: &str) -> Result<bool, RuntimeError> {
    if xml.len() > 1_024 * 1_024 {
        return Err(RuntimeError::ownership("storage-pool XML is oversized"));
    }
    Ok(Document::parse(xml)
        .map_err(RuntimeError::ownership)?
        .descendants()
        .find(|node| node.has_tag_name("pool"))
        .and_then(|node| node.attribute("type"))
        == Some("dir"))
}

#[cfg(test)]
mod tests;
