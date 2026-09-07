use std::time::Instant;

use flanforge_libvirt_wire::OwnershipManifest;
use virt::{connect::Connect, domain::Domain, error::ErrorNumber};

use crate::{
    RuntimeError,
    domain::{DomainSpec, METADATA_URI, domain_xml, ensure_metadata_matches},
};

use super::{
    message::{ActorConfig, DefineRequest},
    storage,
};

mod cleanup;
mod exec;
mod quiesce;

pub(super) use cleanup::cleanup;
pub(super) use exec::{exec, exec_status, is_exec_enabled, ping};
pub(super) use quiesce::{ensure_inactive, quiesce};

pub(super) fn define(
    connection: &Connect,
    config: &ActorConfig,
    request: &DefineRequest,
) -> Result<(), RuntimeError> {
    request
        .manifest
        .ensure_valid()
        .map_err(RuntimeError::manifest)?;
    ensure_absent(connection, request.manifest.domain_name())?;
    ensure_volumes(connection, &config.pool, &request.manifest)?;
    let xml = domain_xml(DomainSpec {
        manifest: &request.manifest,
        pool: &config.pool,
        network: &config.network,
        cpu_count: request.cpu_count,
        memory_mb: request.memory_mb,
    })?;
    let mut domain = Domain::define_xml(connection, &xml)
        .map_err(|error| RuntimeError::libvirt("domain definition", error))?;
    let result = ensure_domain(&domain, &request.manifest);
    if result.is_err() {
        let _ = domain.undefine();
    }
    free_domain(&mut domain)?;
    result
}

pub(super) fn start(
    connection: &Connect,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    let mut domain = exact_domain(connection, manifest)?;
    domain
        .create()
        .map_err(|error| RuntimeError::libvirt("domain start", error))?;
    free_domain(&mut domain)
}

/// Every agent call goes through here, so the ownership check and the transient
/// classification are stated once. `command` is always built by `serde_json`,
/// never `format!`, which is what keeps the wire string NUL-free.
pub(super) fn agent_command(
    connection: &Connect,
    manifest: &OwnershipManifest,
    command: &str,
    deadline: Instant,
) -> Result<String, RuntimeError> {
    let mut domain = exact_domain(connection, manifest)?;
    let timeout = deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .clamp(1, 5) as i32;
    let response = domain
        .qemu_agent_command(command, timeout, 0)
        .map_err(|error| {
            if matches!(
                error.code(),
                ErrorNumber::AgentUnresponsive
                    | ErrorNumber::AgentUnsynced
                    | ErrorNumber::AgentCommandTimeout
                    | ErrorNumber::OperationTimeout
            ) {
                RuntimeError::transient("guest-agent query", error)
            } else {
                RuntimeError::libvirt("guest-agent query", error)
            }
        });
    free_domain(&mut domain)?;
    response
}

pub(super) fn agent_interfaces(
    connection: &Connect,
    manifest: &OwnershipManifest,
    deadline: Instant,
) -> Result<String, RuntimeError> {
    let command = serde_json::to_string(&serde_json::json!({
        "execute": "guest-network-get-interfaces",
    }))
    .map_err(|error| RuntimeError::libvirt("guest-agent command", error))?;
    agent_command(connection, manifest, &command, deadline)
}

fn ensure_absent(connection: &Connect, name: &str) -> Result<(), RuntimeError> {
    match Domain::lookup_by_name(connection, name) {
        Ok(mut domain) => {
            free_domain(&mut domain)?;
            Err(RuntimeError::ownership(
                "refusing to replace an existing libvirt domain",
            ))
        }
        Err(error) if error.code() == ErrorNumber::NoDomain => Ok(()),
        Err(error) => Err(RuntimeError::libvirt("domain collision check", error)),
    }
}

fn ensure_volumes(
    connection: &Connect,
    pool_name: &str,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    storage::ensure_volume(
        connection,
        pool_name,
        manifest
            .overlay()
            .key()
            .ok_or_else(|| RuntimeError::ownership("overlay key is absent"))?,
        manifest.overlay().name(),
    )?;
    storage::ensure_volume(
        connection,
        pool_name,
        manifest
            .seed()
            .key()
            .ok_or_else(|| RuntimeError::ownership("seed key is absent"))?,
        manifest.seed().name(),
    )
}

pub(super) fn exact_domain(
    connection: &Connect,
    manifest: &OwnershipManifest,
) -> Result<Domain, RuntimeError> {
    let domain = Domain::lookup_by_uuid(connection, manifest.domain_uuid())
        .map_err(|error| RuntimeError::libvirt("domain lookup", error))?;
    ensure_domain(&domain, manifest)?;
    Ok(domain)
}

pub(super) fn ensure_domain(
    domain: &Domain,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    let name = domain
        .get_name()
        .map_err(|error| RuntimeError::libvirt("domain name", error))?;
    let uuid = domain
        .get_uuid()
        .map_err(|error| RuntimeError::libvirt("domain UUID", error))?;
    let metadata = domain
        .get_metadata(
            virt::sys::VIR_DOMAIN_METADATA_ELEMENT.cast_signed(),
            Some(METADATA_URI),
            0,
        )
        .map_err(|error| RuntimeError::libvirt("domain metadata", error))?;
    if name != manifest.domain_name() || uuid != manifest.domain_uuid() {
        return Err(RuntimeError::ownership(
            "live domain identity disagrees with its manifest",
        ));
    }
    ensure_metadata_matches(&metadata, manifest)
}

pub(super) fn free_domain(domain: &mut Domain) -> Result<(), RuntimeError> {
    domain
        .free()
        .map_err(|error| RuntimeError::libvirt("domain handle release", error))
}

#[cfg(test)]
mod tests;
