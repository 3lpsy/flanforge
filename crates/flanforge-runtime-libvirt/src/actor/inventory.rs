use flanforge_core::GuestSize;
use flanforge_libvirt_wire::DomainOwnershipMetadata;
use flanforge_manager::{HostMachine, MachineOwnership, MachineState};
use roxmltree::Document;
use virt::{
    connect::Connect, domain::Domain, error::ErrorNumber, storage_pool::StoragePool,
    storage_vol::StorageVol,
};

use crate::{RuntimeError, domain::METADATA_URI};

use super::message::ActorConfig;

pub(super) fn list(
    connection: &Connect,
    config: &ActorConfig,
) -> Result<Vec<HostMachine>, RuntimeError> {
    let mut domains = connection
        .list_all_domains(0)
        .map_err(|error| RuntimeError::libvirt("domain inventory", error))?;
    if domains.len() > 4_096 {
        return Err(RuntimeError::libvirt(
            "domain inventory",
            "host returned too many domains",
        ));
    }
    let mut machines = Vec::with_capacity(domains.len());
    for domain in &mut domains {
        machines.push(machine(connection, domain, config)?);
        domain
            .free()
            .map_err(|error| RuntimeError::libvirt("domain handle release", error))?;
    }
    Ok(machines)
}

fn machine(
    connection: &Connect,
    domain: &Domain,
    config: &ActorConfig,
) -> Result<HostMachine, RuntimeError> {
    let name = domain
        .get_name()
        .map_err(|error| RuntimeError::libvirt("domain inventory name", error))?;
    let info = domain
        .get_info()
        .map_err(|error| RuntimeError::libvirt("domain inventory size", error))?;
    let xml = domain
        .get_xml_desc(0)
        .map_err(|error| RuntimeError::libvirt("domain inventory XML", error))?;
    let size = inventory_size(
        connection,
        &xml,
        info.nr_virt_cpu,
        info.max_mem,
        &config.pool,
    )?;
    Ok(HostMachine {
        name,
        state: state(info.state),
        age_seconds: age_seconds(domain, config),
        size,
        ownership: ownership(domain, config.service_instance),
    })
}

/// The daemon's own ownership manifest records when it created the guest, so a
/// domain it still claims has an age the sweep can act on. A domain no manifest
/// claims — anything foreign — keeps an unknown age and is never a candidate.
fn age_seconds(domain: &Domain, config: &ActorConfig) -> Option<u64> {
    let metadata = domain
        .get_metadata(
            virt::sys::VIR_DOMAIN_METADATA_ELEMENT.cast_signed(),
            Some(METADATA_URI),
            0,
        )
        .ok()?;
    let metadata = DomainOwnershipMetadata::parse(&metadata).ok()?;
    let manifest = crate::manifest::path(&config.state_dir, metadata.allocation_id());
    let modified = std::fs::metadata(manifest).ok()?.modified().ok()?;
    modified.elapsed().ok().map(|elapsed| elapsed.as_secs())
}

fn inventory_size(
    connection: &Connect,
    xml: &str,
    cpu_count: u32,
    memory_kib: u64,
    pool_name: &str,
) -> Result<Option<GuestSize>, RuntimeError> {
    if xml.len() > 1024 * 1024 {
        return Ok(None);
    }
    let document = Document::parse(xml).map_err(RuntimeError::ownership)?;
    let mut pool = StoragePool::lookup_by_name(connection, pool_name)
        .map_err(|error| RuntimeError::libvirt("inventory pool lookup", error))?;
    let mut storage_bytes = 0_u64;
    for disk in document
        .descendants()
        .filter(|node| node.has_tag_name("disk") && node.attribute("device") == Some("disk"))
    {
        let Some(source) = disk.children().find(|node| node.has_tag_name("source")) else {
            let _ = pool.free();
            return Ok(None);
        };
        if source.attribute("pool") != Some(pool_name) {
            let _ = pool.free();
            return Ok(None);
        }
        let Some(name) = source.attribute("volume") else {
            let _ = pool.free();
            return Ok(None);
        };
        let Ok(mut volume) = StorageVol::lookup_by_name(&pool, name) else {
            let _ = pool.free();
            return Ok(None);
        };
        let info = volume
            .get_info()
            .map_err(|error| RuntimeError::libvirt("inventory volume size", error))?;
        storage_bytes = storage_bytes.saturating_add(info.capacity);
        volume
            .free()
            .map_err(|error| RuntimeError::libvirt("volume handle release", error))?;
    }
    pool.free()
        .map_err(|error| RuntimeError::libvirt("pool handle release", error))?;
    let Ok(cpu_count) = u8::try_from(cpu_count) else {
        return Ok(None);
    };
    let Ok(memory_mb) = u32::try_from(memory_kib.saturating_add(1_023) / 1_024) else {
        return Ok(None);
    };
    let storage_mb = storage_bytes.saturating_add(1_048_575) / 1_048_576;
    Ok(Some(GuestSize {
        cpu_count,
        memory_mb,
        storage_mb,
    }))
}

fn ownership(domain: &Domain, service_instance: uuid::Uuid) -> MachineOwnership {
    match domain.get_metadata(
        virt::sys::VIR_DOMAIN_METADATA_ELEMENT.cast_signed(),
        Some(METADATA_URI),
        0,
    ) {
        Ok(metadata) => {
            let Ok(metadata) = DomainOwnershipMetadata::parse(&metadata) else {
                return MachineOwnership::Unknown;
            };
            let Ok(domain_uuid) = domain.get_uuid() else {
                return MachineOwnership::Unknown;
            };
            classify_metadata(&metadata, domain_uuid, service_instance)
        }
        Err(error) if error.code() == ErrorNumber::NoDomainMetadata => MachineOwnership::Foreign,
        Err(_) => MachineOwnership::Unknown,
    }
}

fn classify_metadata(
    metadata: &DomainOwnershipMetadata,
    live_domain_uuid: uuid::Uuid,
    service_instance: uuid::Uuid,
) -> MachineOwnership {
    if metadata.domain_uuid() != live_domain_uuid {
        MachineOwnership::Unknown
    } else if metadata.service_instance() == service_instance {
        MachineOwnership::Owned
    } else {
        MachineOwnership::Foreign
    }
}

fn state(state: virt::sys::virDomainState) -> MachineState {
    match state {
        virt::sys::VIR_DOMAIN_RUNNING | virt::sys::VIR_DOMAIN_BLOCKED => MachineState::Running,
        virt::sys::VIR_DOMAIN_SHUTOFF | virt::sys::VIR_DOMAIN_SHUTDOWN => MachineState::Stopped,
        _ => MachineState::Other,
    }
}

#[cfg(test)]
mod tests;
