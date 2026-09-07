use std::os::unix::fs::PermissionsExt;

use flanforge_core::{Config, LibvirtConfig};
use flanforge_store::StateMutationLock;

use crate::{RuntimeError, actor::ActorConfig, seed::operator_public_key};

pub(crate) fn backend(config: &Config) -> Result<&LibvirtConfig, RuntimeError> {
    config
        .ensure_valid()
        .map_err(|error| RuntimeError::Configuration {
            message: error.to_string(),
        })?;
    config
        .runtime
        .libvirt()
        .ok_or_else(|| RuntimeError::Configuration {
            message: "operation requires the libvirt backend".to_owned(),
        })
}

pub(crate) fn actor_config(
    config: &Config,
    service_instance: uuid::Uuid,
) -> Result<ActorConfig, RuntimeError> {
    let backend = backend(config)?;
    Ok(ActorConfig {
        uri: backend.uri.clone(),
        pool: backend.pool.clone(),
        network: backend.network.clone(),
        state_dir: config.runtime.state_dir.clone(),
        service_instance,
        min_storage_free_bytes: backend
            .min_storage_free_mb
            .checked_mul(1_048_576)
            .ok_or_else(|| RuntimeError::Configuration {
                message: "storage reserve overflows bytes".to_owned(),
            })?,
        allow_insecure_transport: backend.allow_insecure_transport,
        is_warm_declared: config
            .profiles
            .values()
            .any(|profile| profile.warm_template.is_some()),
    })
}

pub(crate) fn ensure_lock(config: &Config, lock: &StateMutationLock) -> Result<(), RuntimeError> {
    if lock.is_for(&config.runtime.state_dir) {
        Ok(())
    } else {
        Err(RuntimeError::Configuration {
            message: "mutation lock belongs to another state directory".to_owned(),
        })
    }
}

/// The public half of the configured guest identity, or `None` when no
/// `[guest.ssh]` table exists at all — which the agent channel admits, and
/// which then produces a seed carrying no key material whatsoever.
pub(crate) async fn read_operator_public_key(
    config: &Config,
) -> Result<Option<String>, RuntimeError> {
    let Some(ssh) = config.guest.ssh.as_ref() else {
        return Ok(None);
    };
    read_public_key_file(&ssh.identity_file).await.map(Some)
}

/// The public half of the privileged account's identity, falling back to the
/// job identity so one key can drive both accounts.
pub(crate) async fn read_privileged_public_key(
    config: &Config,
) -> Result<Option<String>, RuntimeError> {
    let Some(ssh) = config.guest.ssh.as_ref() else {
        return Ok(None);
    };
    let path = ssh
        .privileged_identity_file
        .as_ref()
        .unwrap_or(&ssh.identity_file);
    read_public_key_file(path).await.map(Some)
}

async fn read_public_key_file(path: &std::path::Path) -> Result<String, RuntimeError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(RuntimeError::seed)?;
    if !metadata.file_type().is_file()
        || metadata.len() > 64 * 1_024
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(RuntimeError::seed(
            "guest SSH identity must be a private bounded regular file",
        ));
    }
    let key = tokio::fs::read(path).await.map_err(RuntimeError::seed)?;
    operator_public_key(&key)
}
