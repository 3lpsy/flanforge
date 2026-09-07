use std::path::Path;

use flanforge_core::{Allocation, VmName};
use flanforge_libvirt_wire::{Artifact, OwnershipManifest};
use uuid::Uuid;

use crate::RuntimeError;

use super::{super::paths, ServiceInstance};

pub(crate) fn intent(
    allocation: &Allocation,
    instance: &ServiceInstance,
    state_dir: &Path,
    host_key_alias: String,
) -> Result<OwnershipManifest, RuntimeError> {
    intent_for(
        allocation.id.into_uuid(),
        &allocation.vm_name,
        instance,
        state_dir,
        host_key_alias,
    )
}

pub(crate) fn intent_for(
    allocation_id: Uuid,
    vm_name: &VmName,
    instance: &ServiceInstance,
    state_dir: &Path,
    host_key_alias: String,
) -> Result<OwnershipManifest, RuntimeError> {
    let artifact_id = Uuid::new_v4();
    let manifest = OwnershipManifest::new(
        allocation_id,
        instance.id(),
        vm_name.as_str().to_owned(),
        Uuid::new_v4(),
        mac_for(allocation_id),
        Artifact::new(format!("root-{artifact_id}.qcow2")),
        Artifact::new(format!("seed-{artifact_id}.img")),
        host_key_alias,
        paths::known_hosts(state_dir, allocation_id),
        flanforge_utils::try_unix_time().map_err(RuntimeError::manifest)?,
    );
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    Ok(manifest)
}

/// The reap variant of `ensure_matches`, deliberately without the service
/// instance check. A guest that outlived the process that made it carries a
/// previous instance id, and that is precisely what the sweep exists to
/// collect; the durable allocation record is what authorizes the deletion.
pub(crate) fn ensure_reapable(
    manifest: &OwnershipManifest,
    allocation_id: Uuid,
    vm_name: &VmName,
    state_dir: &Path,
) -> Result<(), RuntimeError> {
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    if manifest.allocation_id() != allocation_id
        || manifest.domain_name() != vm_name.as_str()
        || manifest.known_hosts_file() != paths::known_hosts(state_dir, allocation_id)
    {
        return Err(RuntimeError::ownership(
            "durable allocation and libvirt manifest disagree",
        ));
    }
    ensure_recovery_artifacts(manifest)
}

pub(crate) fn ensure_matches(
    manifest: &OwnershipManifest,
    allocation_id: Uuid,
    vm_name: &VmName,
    instance: &ServiceInstance,
    state_dir: &Path,
) -> Result<(), RuntimeError> {
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    if manifest.allocation_id() != allocation_id
        || manifest.domain_name() != vm_name.as_str()
        || manifest.service_instance() != instance.id()
        || manifest.known_hosts_file() != paths::known_hosts(state_dir, allocation_id)
    {
        return Err(RuntimeError::ownership(
            "durable allocation and libvirt manifest disagree",
        ));
    }
    ensure_recovery_artifacts(manifest)
}

pub(crate) fn ensure_recovery_artifacts(manifest: &OwnershipManifest) -> Result<(), RuntimeError> {
    let overlay = artifact_id(manifest.overlay().name(), "root-", ".qcow2");
    let seed = artifact_id(manifest.seed().name(), "seed-", ".img");
    if overlay.is_none() || overlay != seed {
        return Err(RuntimeError::ownership(
            "durable allocation artifact identities disagree",
        ));
    }
    Ok(())
}

fn artifact_id(value: &str, prefix: &str, suffix: &str) -> Option<Uuid> {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
}

pub(crate) fn mac_for(allocation_id: Uuid) -> String {
    let bytes = allocation_id.as_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}
