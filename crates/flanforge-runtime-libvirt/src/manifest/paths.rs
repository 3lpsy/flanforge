use std::path::{Path, PathBuf};

use uuid::Uuid;

pub(super) const KNOWN_HOSTS_FILE: &str = "known_hosts";
pub(super) const OWNERSHIP_FILE: &str = "ownership.json";
pub(super) const CLEANUP_FILE: &str = "cleanup-complete.json";

pub(super) fn service_instance(state_dir: &Path) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir).service_instance
}

pub(crate) fn allocation_dir(state_dir: &Path, allocation_id: Uuid) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir)
        .allocations
        .join(allocation_id.to_string())
}

pub(crate) fn ownership(state_dir: &Path, allocation_id: Uuid) -> PathBuf {
    allocation_dir(state_dir, allocation_id).join(OWNERSHIP_FILE)
}

pub(crate) fn cleanup(state_dir: &Path, allocation_id: Uuid) -> PathBuf {
    allocation_dir(state_dir, allocation_id).join(CLEANUP_FILE)
}

pub(super) fn known_hosts(state_dir: &Path, allocation_id: Uuid) -> PathBuf {
    allocation_dir(state_dir, allocation_id).join(KNOWN_HOSTS_FILE)
}

pub(crate) fn is_cleanup_temporary_name(name: &str) -> bool {
    name.strip_prefix(".cleanup-")
        .and_then(|value| value.strip_suffix(".tmp"))
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
}

pub(crate) fn is_ownership_temporary_name(name: &str) -> bool {
    name.strip_prefix(".ownership-")
        .and_then(|value| value.strip_suffix(".tmp"))
        .or_else(|| {
            name.strip_prefix("ownership.")
                .and_then(|value| value.strip_suffix(".tmp"))
        })
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
}
