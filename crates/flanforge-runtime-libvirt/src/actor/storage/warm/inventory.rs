mod model;

use roxmltree::Document;
use virt::{
    connect::Connect, domain::Domain, error::ErrorNumber, storage_pool::StoragePool,
    storage_vol::StorageVol,
};

use crate::{
    RuntimeError,
    file::{HEADER_WINDOW_BYTES, normalize_path, parse_header},
};

pub(super) use model::BackingInventory;

use super::super::handles::{free_pool, free_volume};
use super::stream::download_prefix;

/// Matching the domain-inventory guard, so one pathological pool cannot make
/// this walk unbounded.
const MAX_VOLUMES: usize = 4_096;
const MAX_DOMAINS: usize = 4_096;
const MAX_XML_BYTES: usize = 1_024 * 1_024;
/// A chain deeper than this is refused rather than followed: our own overlays
/// are one deep, and a cycle must not become an unbounded walk.
const MAX_CHAIN_DEPTH: usize = 8;

/// Walks every volume in the configured pool and reports what each names as
/// its backing file.
///
/// Two independent instruments are read for every volume: the qcow2 header, by
/// streaming a bounded prefix of the file itself, and libvirt's cached volume
/// XML. A missing `<backingStore>` element is inconclusive, never absence, so
/// the two are required to agree — a file libvirt could not probe, or a chain
/// it could not stat, refuses the whole proof rather than clearing a
/// generation a live guest is still reading.
pub(super) fn referenced_backing_paths(
    connection: &Connect,
    pool_name: &str,
) -> Result<BackingInventory, RuntimeError> {
    let mut pool = StoragePool::lookup_by_name(connection, pool_name)
        .map_err(|error| RuntimeError::libvirt("retirement pool lookup", error))?;
    // Discards libvirt's cached volume objects and rebuilds them by probing,
    // so the walk sees volumes another writer created since this connection.
    let refreshed = pool
        .refresh(0)
        .map_err(|error| RuntimeError::libvirt("retirement pool refresh", error));
    let volumes = refreshed.and_then(|_| {
        pool.list_all_volumes(0)
            .map_err(|error| RuntimeError::libvirt("retirement volume inventory", error))
    });
    let mut volumes = match volumes {
        Ok(volumes) => volumes,
        Err(error) => {
            let _ = free_pool(&mut pool);
            return Err(error);
        }
    };
    let result = (|| {
        if volumes.len() > MAX_VOLUMES {
            return Err(RuntimeError::libvirt(
                "retirement volume inventory",
                "pool returned too many volumes",
            ));
        }
        let mut inventory = BackingInventory::default();
        for volume in &volumes {
            if let Some(path) = backing_path(connection, volume)? {
                inventory.insert(connection, &path)?;
            }
        }
        Ok(inventory)
    })();
    for volume in &mut volumes {
        free_volume(volume)?;
    }
    free_pool(&mut pool)?;
    result
}

/// Adds every backing file a defined domain names, including one whose disk
/// lives outside the configured pool.
///
/// A domain's XML is read for the whole chain it declares, and every disk
/// libvirt can resolve to a volume is asked what it is backed by as well: an
/// inactive domain's persistent XML carries no `<backingStore>`, so its
/// declaration alone would name only the overlay.
pub(super) fn ensure_domains_walked(
    connection: &Connect,
    inventory: &mut BackingInventory,
) -> Result<(), RuntimeError> {
    let mut domains = connection
        .list_all_domains(0)
        .map_err(|error| RuntimeError::libvirt("retirement domain inventory", error))?;
    let result = (|| {
        if domains.len() > MAX_DOMAINS {
            return Err(RuntimeError::libvirt(
                "retirement domain inventory",
                "host returned too many domains",
            ));
        }
        for domain in &domains {
            let disks = domain_disks(domain)?;
            for path in disks.paths {
                inventory.insert(connection, &path)?;
                ensure_chain_walked(connection, inventory, &path)?;
            }
            for (pool, volume) in disks.volumes {
                if let Some(path) = inventory.insert_volume(connection, &pool, &volume)? {
                    ensure_chain_walked(connection, inventory, &path)?;
                }
            }
        }
        Ok(())
    })();
    for domain in &mut domains {
        domain
            .free()
            .map_err(|error| RuntimeError::libvirt("domain handle release", error))?;
    }
    result
}

/// Follows what a domain disk is actually backed by, one volume at a time, for
/// as far as libvirt can resolve. A path no pool knows ends the walk: nothing
/// can be read about it, and it is already recorded by path.
fn ensure_chain_walked(
    connection: &Connect,
    inventory: &mut BackingInventory,
    path: &str,
) -> Result<(), RuntimeError> {
    let mut current = path.to_owned();
    for _ in 0..MAX_CHAIN_DEPTH {
        let Some(parent) = resolved_backing_path(connection, &current)? else {
            return Ok(());
        };
        inventory.insert(connection, &parent)?;
        current = parent;
    }
    Err(RuntimeError::ownership(
        "domain disk backing chain is too deep to prove",
    ))
}

fn resolved_backing_path(connection: &Connect, path: &str) -> Result<Option<String>, RuntimeError> {
    let mut volume = match StorageVol::lookup_by_path(connection, path) {
        Ok(volume) => volume,
        Err(error) if error.code() == ErrorNumber::NoStorageVolume => return Ok(None),
        Err(error) => return Err(RuntimeError::libvirt("disk volume lookup", error)),
    };
    let backing = backing_path(connection, &volume);
    free_volume(&mut volume)?;
    backing
}

/// Reads one volume's backing file from its own bytes and from libvirt, and
/// refuses whenever the two disagree.
fn backing_path(connection: &Connect, volume: &StorageVol) -> Result<Option<String>, RuntimeError> {
    let info = volume
        .get_info()
        .map_err(|error| RuntimeError::libvirt("retirement volume info", error))?;
    let prefix = download_prefix(connection, volume, info.allocation.min(HEADER_WINDOW_BYTES))?;
    let header = parse_header(&prefix)?;
    let xml = volume
        .get_xml_desc(0)
        .map_err(|error| RuntimeError::libvirt("retirement volume XML", error))?;
    let declared = xml_backing_path(&xml)?;
    reconcile_views(header.and_then(|header| header.backing_file), declared)
}

/// A missing `<backingStore>` element is inconclusive, never absence: a file
/// libvirt could not probe, a pool directory reached through a symlink, or a
/// chain it could not stat all produce one. So the two views must agree, and
/// disagreement refuses rather than clearing a generation a live guest reads.
fn reconcile_views(
    recorded: Option<String>,
    declared: Option<String>,
) -> Result<Option<String>, RuntimeError> {
    match (recorded, declared) {
        (None, None) => Ok(None),
        (Some(recorded), Some(declared))
            if normalize_path(&recorded) == normalize_path(&declared) =>
        {
            Ok(Some(recorded))
        }
        _ => Err(RuntimeError::ownership(
            "volume backing-file views disagree; refusing to prove anything unreferenced",
        )),
    }
}

fn xml_backing_path(xml: &str) -> Result<Option<String>, RuntimeError> {
    if xml.len() > MAX_XML_BYTES {
        return Err(RuntimeError::ownership("volume XML is oversized"));
    }
    let document = Document::parse(xml).map_err(RuntimeError::ownership)?;
    Ok(document
        .descendants()
        .find(|node| node.has_tag_name("backingStore"))
        .and_then(|node| node.children().find(|child| child.has_tag_name("path")))
        .and_then(|node| node.text())
        .map(ToOwned::to_owned))
}

/// Every disk a domain names: file-typed sources directly, and pool-typed
/// sources by the volume they resolve to. A domain that attached a generation
/// as a disk rather than as a backing file is a reference either way.
#[derive(Debug, Default, Eq, PartialEq)]
struct DomainDisks {
    paths: Vec<String>,
    volumes: Vec<(String, String)>,
}

fn domain_disks(domain: &Domain) -> Result<DomainDisks, RuntimeError> {
    let xml = domain
        .get_xml_desc(0)
        .map_err(|error| RuntimeError::libvirt("retirement domain XML", error))?;
    parse_domain_disks(&xml)
}

fn parse_domain_disks(xml: &str) -> Result<DomainDisks, RuntimeError> {
    if xml.len() > MAX_XML_BYTES {
        return Err(RuntimeError::ownership("domain XML is oversized"));
    }
    let document = Document::parse(xml).map_err(RuntimeError::ownership)?;
    let mut disks = DomainDisks::default();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("source"))
    {
        if let Some(path) = node.attribute("file").or_else(|| node.attribute("dev")) {
            disks.paths.push(path.to_owned());
        }
        if let (Some(pool), Some(volume)) = (node.attribute("pool"), node.attribute("volume")) {
            disks.volumes.push((pool.to_owned(), volume.to_owned()));
        }
    }
    Ok(disks)
}

#[cfg(test)]
mod tests;
