use super::{
    limits::{MAX_ARTIFACT_KEY_BYTES, MAX_KNOWN_HOSTS_PATH_BYTES, MAX_SAFE_NAME_BYTES},
    model::OwnershipManifest,
};
use crate::{
    WireError,
    validation::{is_normal_absolute, is_safe_key, is_safe_name},
};

const CONTRACT: &str = "libvirt ownership manifest";

pub(super) fn ensure_valid(manifest: &OwnershipManifest) -> Result<(), WireError> {
    if manifest.schema_version != 1 {
        return invalid("schema_version");
    }
    if manifest.allocation_id.is_nil() {
        return invalid("allocation_id");
    }
    if manifest.service_instance.is_nil() {
        return invalid("service_instance");
    }
    if manifest.domain_uuid.is_nil() {
        return invalid("domain_uuid");
    }
    if manifest.created_unix_seconds == 0 {
        return invalid("created_unix_seconds");
    }
    for (field, value) in [
        ("domain_name", manifest.domain_name.as_str()),
        ("overlay.name", manifest.overlay.name.as_str()),
        ("seed.name", manifest.seed.name.as_str()),
    ] {
        if !is_safe_name(value, MAX_SAFE_NAME_BYTES) {
            return invalid(field);
        }
    }
    if !is_mac(&manifest.mac_address) {
        return invalid("mac_address");
    }
    if !is_safe_name(&manifest.host_key_alias, MAX_SAFE_NAME_BYTES) {
        return invalid("host_key_alias");
    }
    if !is_normal_absolute(&manifest.known_hosts_file, MAX_KNOWN_HOSTS_PATH_BYTES) {
        return invalid("known_hosts_file");
    }
    for (field, key) in [
        ("overlay.key", manifest.overlay.key.as_deref()),
        ("seed.key", manifest.seed.key.as_deref()),
    ] {
        if key.is_some_and(|value| !is_safe_key(value, MAX_ARTIFACT_KEY_BYTES)) {
            return invalid(field);
        }
    }
    Ok(())
}

fn is_mac(value: &str) -> bool {
    value.len() == 17
        && value.split(':').count() == 6
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}
