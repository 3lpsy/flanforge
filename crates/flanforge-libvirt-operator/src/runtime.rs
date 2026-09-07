use flanforge_core::{Config, ProfileName};
use tokio_util::sync::CancellationToken;

use crate::{
    OperatorError, SmokeReport,
    image::{ensure_config, ensure_not_cancelled},
};

/// Runs one profile-sized disposable guest and always cleans its exact assets.
///
/// # Errors
/// Returns an error for an unknown profile, lock contention, lifecycle failure,
/// cancellation, or failed exact cleanup.
pub async fn smoke(
    config: &Config,
    profile_name: &ProfileName,
    cancellation: &CancellationToken,
) -> Result<SmokeReport, OperatorError> {
    ensure_config(config)?;
    let profile = config
        .profiles
        .get(profile_name)
        .ok_or_else(|| OperatorError::ProfileNotFound(profile_name.clone()))?;
    ensure_not_cancelled(cancellation)?;
    let lock = flanforge_store::StateMutationLock::acquire(&config.runtime.state_dir).await?;
    ensure_not_cancelled(cancellation)?;
    flanforge_runtime_libvirt::smoke_disposable(config, &lock, profile, cancellation)
        .await
        .map(SmokeReport)
        .map_err(Into::into)
}
