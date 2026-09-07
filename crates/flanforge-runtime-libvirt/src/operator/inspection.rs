use std::path::Path;

use flanforge_core::Config;

use crate::{
    RuntimeError,
    context::backend,
    image::{inspect_qcow2, verify_image},
};

use super::BaseImageInspection;

/// Verifies a local base image without changing host or runtime state.
///
/// # Errors
/// Returns an error for invalid configuration, unsafe files, digest mismatch,
/// or an incompatible qcow2 image.
pub async fn inspect_base_image(
    config: &Config,
    image_path: &Path,
    manifest_path: Option<&Path>,
) -> Result<BaseImageInspection, RuntimeError> {
    let backend = backend(config)?;
    let verified = verify_image(image_path, manifest_path, &backend.qemu_img_path).await?;
    inspect_qcow2(image_path, &backend.qemu_img_path, &verified.manifest).await?;
    Ok(BaseImageInspection::from_manifest(&verified.manifest))
}
