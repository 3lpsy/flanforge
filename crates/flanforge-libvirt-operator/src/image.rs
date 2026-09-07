use std::path::Path;

use flanforge_core::{Config, VmName};
use tokio_util::sync::CancellationToken;

use crate::{ImageImportReport, ImageInspection, OperatorError};

/// Verifies a local image and manifest without acquiring the mutation lock.
///
/// # Errors
/// Returns an error for invalid configuration or image content.
pub async fn inspect(
    config: &Config,
    image_path: &Path,
    manifest_path: Option<&Path>,
) -> Result<ImageInspection, OperatorError> {
    ensure_config(config)?;
    flanforge_runtime_libvirt::inspect_base_image(config, image_path, manifest_path)
        .await
        .map(ImageInspection)
        .map_err(Into::into)
}

/// Imports and immutably publishes one verified base under the shared lock.
///
/// Cancellation is accepted only before the import mutation begins.
///
/// # Errors
/// Returns an error for cancellation, lock contention, or import failure.
pub async fn import(
    config: &Config,
    logical_name: &VmName,
    image_path: &Path,
    manifest_path: Option<&Path>,
    cancellation: &CancellationToken,
) -> Result<ImageImportReport, OperatorError> {
    ensure_config(config)?;
    ensure_not_cancelled(cancellation)?;
    let lock = flanforge_store::StateMutationLock::acquire(&config.runtime.state_dir).await?;
    ensure_not_cancelled(cancellation)?;
    let base = flanforge_runtime_libvirt::import_base(
        config,
        &lock,
        logical_name,
        image_path,
        manifest_path,
        false,
    )
    .await?;
    Ok(ImageImportReport::new(logical_name.clone(), &base))
}

pub(super) fn ensure_config(config: &Config) -> Result<(), OperatorError> {
    config
        .ensure_valid()
        .map_err(|error| OperatorError::Configuration {
            message: error.to_string(),
        })?;
    if config.runtime.libvirt().is_none() {
        return Err(OperatorError::Configuration {
            message: "operation requires the libvirt backend".to_owned(),
        });
    }
    Ok(())
}

pub(super) fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), OperatorError> {
    if cancellation.is_cancelled() {
        Err(OperatorError::Cancelled)
    } else {
        Ok(())
    }
}
