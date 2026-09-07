use uuid::Uuid;

use crate::{
    WireError,
    libvirt_helper::{
        limits::{MAX_HELPER_PATH_BYTES, MAX_SAFE_NAME_BYTES},
        model::{HelperConfig, HelperRequest},
    },
    validation::{is_normal_absolute, is_safe_name},
};

pub(super) const CONTRACT: &str = "libvirt helper";

/// One pool cannot hold more warm generations than every profile's structural
/// cap put together; this bounds the request rather than the policy.
pub(super) const MAX_RETIREMENT_POINTERS: usize = 512;

pub(super) fn config(request: &HelperRequest) -> &HelperConfig {
    match request {
        HelperRequest::Probe { config }
        | HelperRequest::Inventory { config }
        | HelperRequest::CheckSource { config, .. }
        | HelperRequest::WarmQuiesce { config, .. }
        | HelperRequest::WarmCapture { config, .. }
        | HelperRequest::WarmVerify { config, .. }
        | HelperRequest::WarmRetire { config, .. }
        | HelperRequest::Start { config, .. }
        | HelperRequest::Address { config, .. }
        | HelperRequest::AgentProbe { config, .. }
        | HelperRequest::AgentExec { config, .. }
        | HelperRequest::AgentExecStatus { config, .. }
        | HelperRequest::Cleanup { config, .. }
        | HelperRequest::DeleteVolume { config, .. }
        | HelperRequest::Create { config, .. }
        | HelperRequest::Define { config, .. }
        | HelperRequest::Import { config, .. } => config,
    }
}

pub(super) fn ensure_config(config: &HelperConfig) -> Result<(), WireError> {
    if !flanforge_utils::is_supported_libvirt_uri(&config.uri, config.allow_insecure_transport)
        || !is_safe_name(&config.pool, MAX_SAFE_NAME_BYTES)
        || !is_safe_name(&config.network, MAX_SAFE_NAME_BYTES)
        || !is_normal_absolute(&config.state_dir, MAX_HELPER_PATH_BYTES)
        || config.service_instance.is_nil()
        || config.min_storage_free_bytes == 0
    {
        return invalid("config");
    }
    Ok(())
}

pub(super) fn ensure_capture(
    profile: &str,
    capture_id: Uuid,
    generation: u64,
    virtual_bytes: u64,
) -> Result<(), WireError> {
    if !is_safe_name(profile, MAX_SAFE_NAME_BYTES)
        || capture_id.is_nil()
        || generation == 0
        || virtual_bytes == 0
    {
        return invalid("warm capture");
    }
    Ok(())
}

pub(super) fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}
