use flanforge_core::{AllocationRequest, Config, Profile, VmName};

use super::ManagerError;

/// Owned, because the configuration behind it can be replaced.
pub(crate) fn profile(
    config: &Config,
    request: &AllocationRequest,
) -> Result<Profile, ManagerError> {
    config
        .profiles
        .get(&request.profile)
        .cloned()
        .ok_or_else(|| ManagerError::UnknownProfile(request.profile.to_string()))
}

/// The one VM name an allocation may ever own, derived from run identity.
pub(crate) fn vm_name(
    config: &Config,
    request: &AllocationRequest,
) -> Result<VmName, ManagerError> {
    VmName::new(format!(
        "{}{}-{}-{}",
        config.runtime.vm_prefix, request.profile, request.run_id, request.run_attempt
    ))
    .map_err(|error| ManagerError::InvalidVmName(error.to_string()))
}
