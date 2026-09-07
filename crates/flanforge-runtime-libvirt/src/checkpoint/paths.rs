use std::path::{Path, PathBuf};

use uuid::Uuid;

pub(crate) fn allocation_path(state_dir: &Path, allocation_id: Uuid) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir)
        .allocations
        .join(allocation_id.to_string())
        .join("volume-checkpoint.json")
}

pub(crate) fn import_path(state_dir: &Path, import_id: Uuid) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir)
        .imports
        .join(format!("import-{import_id}.volume-checkpoint.json"))
}

pub(crate) fn warm_path(state_dir: &Path, capture_id: Uuid) -> PathBuf {
    warm_capture_dir(state_dir).join(format!("capture-{capture_id}.volume-checkpoint.json"))
}

pub(crate) fn warm_capture_dir(state_dir: &Path) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir)
        .warm
        .join("captures")
}

/// The capture id a warm checkpoint filename names, so recovery can walk the
/// directory without opening anything first.
pub(crate) fn warm_capture_id(file_name: &str) -> Option<Uuid> {
    file_name
        .strip_prefix("capture-")
        .and_then(|value| value.strip_suffix(".volume-checkpoint.json"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
}

pub(crate) fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix(".volume-checkpoint-")
        .and_then(|value| value.strip_suffix(".tmp"))
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
}
