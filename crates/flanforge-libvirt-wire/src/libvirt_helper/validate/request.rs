use std::path::Path;

use uuid::Uuid;

use crate::{
    WireError,
    libvirt_helper::{
        limits::{
            MAX_HELPER_PATH_BYTES, MAX_SAFE_NAME_BYTES, MAX_SEED_BYTES, MAX_VOLUME_KEY_BYTES,
        },
        model::{HelperConfig, HelperRequest},
    },
    validation::{is_normal_absolute, is_safe_key, is_safe_name},
};

use super::{
    agent::{ensure_exec_valid, ensure_probe_valid, ensure_status_valid},
    shared::{MAX_RETIREMENT_POINTERS, config, ensure_capture, ensure_config, invalid},
};

pub(in crate::libvirt_helper) fn request(request: &HelperRequest) -> Result<(), WireError> {
    let config = config(request);
    ensure_config(config)?;
    match request {
        HelperRequest::Probe { .. } | HelperRequest::Inventory { .. } => Ok(()),
        HelperRequest::CheckSource { pointer, .. } => pointer.ensure_valid(),
        HelperRequest::Import {
            logical_name,
            volume_name,
            staged_image_path,
            manifest,
            ..
        } => ensure_import(
            config,
            logical_name,
            volume_name,
            staged_image_path,
            manifest,
        ),
        HelperRequest::DeleteVolume { key, name, .. } => {
            if key
                .as_deref()
                .is_some_and(|key| !is_safe_key(key, MAX_VOLUME_KEY_BYTES))
                || !is_safe_name(name, MAX_SAFE_NAME_BYTES)
            {
                return invalid("delete volume");
            }
            Ok(())
        }
        request => ensure_guest(request).and_then(|()| ensure_warm(request)),
    }
}

/// Everything bound to one allocation's ownership manifest.
fn ensure_guest(request: &HelperRequest) -> Result<(), WireError> {
    match request {
        HelperRequest::Create {
            manifest,
            source,
            seed,
            storage_bytes,
            ..
        } => {
            manifest.ensure_valid()?;
            source.ensure_valid()?;
            if seed.is_empty() || seed.len() > MAX_SEED_BYTES || *storage_bytes == 0 {
                return invalid("create");
            }
            Ok(())
        }
        HelperRequest::Define {
            manifest,
            cpu_count,
            memory_mb,
            ..
        } => {
            manifest.ensure_valid()?;
            if *cpu_count == 0 || *memory_mb < 2_048 {
                return invalid("define");
            }
            Ok(())
        }
        HelperRequest::Start { manifest, .. } => manifest.ensure_valid(),
        HelperRequest::AgentProbe {
            manifest,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            ensure_probe_valid(*timeout_seconds)
        }
        HelperRequest::AgentExec {
            manifest,
            request,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            ensure_exec_valid(request, *timeout_seconds)
        }
        HelperRequest::AgentExecStatus {
            manifest,
            pid,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            ensure_status_valid(*pid, *timeout_seconds)
        }
        HelperRequest::Address {
            manifest,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            if !(1..=5).contains(timeout_seconds) {
                return invalid("address timeout");
            }
            Ok(())
        }
        HelperRequest::Cleanup {
            manifest,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            if !(1..=600).contains(timeout_seconds) {
                return invalid("cleanup timeout");
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ensure_warm(request: &HelperRequest) -> Result<(), WireError> {
    match request {
        HelperRequest::WarmQuiesce {
            manifest,
            timeout_seconds,
            ..
        } => {
            manifest.ensure_valid()?;
            if !(1..=600).contains(timeout_seconds) {
                return invalid("quiesce timeout");
            }
            Ok(())
        }
        HelperRequest::WarmCapture {
            manifest,
            profile,
            capture_id,
            generation,
            virtual_bytes,
            ..
        } => {
            manifest.ensure_valid()?;
            ensure_capture(profile, *capture_id, *generation, *virtual_bytes)
        }
        HelperRequest::WarmVerify {
            profile,
            capture_id,
            generation,
            virtual_bytes,
            ..
        } => ensure_capture(profile, *capture_id, *generation, *virtual_bytes),
        HelperRequest::WarmRetire {
            candidates,
            protected,
            ..
        } => {
            if candidates.is_empty()
                || candidates.len() > MAX_RETIREMENT_POINTERS
                || protected.len() > MAX_RETIREMENT_POINTERS
            {
                return invalid("retire");
            }
            for pointer in candidates.iter().chain(protected) {
                pointer.ensure_valid()?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ensure_import(
    config: &HelperConfig,
    logical_name: &str,
    volume_name: &str,
    staged_image_path: &Path,
    manifest: &crate::BaseImageManifest,
) -> Result<(), WireError> {
    let Some(import_id) = import_id(volume_name) else {
        return invalid("import volume_name");
    };
    let expected_path = config
        .state_dir
        .join("libvirt")
        .join("imports")
        .join(format!("import-{import_id}.qcow2"));
    if !is_safe_name(logical_name, MAX_SAFE_NAME_BYTES)
        || !is_normal_absolute(staged_image_path, MAX_HELPER_PATH_BYTES)
        || staged_image_path != expected_path
    {
        return invalid("import");
    }
    manifest.ensure_valid()
}

fn import_id(volume_name: &str) -> Option<Uuid> {
    volume_name
        .strip_prefix("base-import-")
        .and_then(|value| value.strip_suffix(".qcow2"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
}
