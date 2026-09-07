use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use flanforge_libvirt_wire::BaseImageManifest;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::RuntimeError;

use super::VerifiedImage;

#[derive(Debug)]
pub(crate) struct StagedImage {
    pub(crate) path: PathBuf,
    pub(crate) manifest: BaseImageManifest,
    pub(crate) volume_name: String,
}

pub(crate) async fn stage(
    image: VerifiedImage,
    state_dir: &Path,
) -> Result<StagedImage, RuntimeError> {
    let directory = state_dir.join("libvirt").join("imports");
    tokio::task::spawn_blocking(move || stage_blocking(image, &directory))
        .await
        .map_err(|_| RuntimeError::manifest("image staging task failed"))?
}

fn stage_blocking(mut image: VerifiedImage, directory: &Path) -> Result<StagedImage, RuntimeError> {
    ensure_private_directory(directory)?;
    let id = Uuid::new_v4();
    let path = directory.join(format!("import-{id}.qcow2"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(RuntimeError::manifest)?;
    let result = copy_verified(&mut image.file, &mut output, &image.manifest);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    output.sync_all().map_err(RuntimeError::manifest)?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(RuntimeError::manifest)?;
    Ok(StagedImage {
        path,
        manifest: image.manifest,
        volume_name: format!("base-import-{id}.qcow2"),
    })
}

// The octal mask names the group and other bits a reader checks for; a
// trailing-zero count does not.
#[allow(clippy::verbose_bit_mask)]
pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_dir() && metadata.permissions().mode() & 0o077 == 0 =>
        {
            Ok(())
        }
        Ok(_) => Err(RuntimeError::manifest(
            "image staging path is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700).recursive(true);
            builder.create(path).map_err(RuntimeError::manifest)
        }
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn copy_verified(
    source: &mut File,
    destination: &mut File,
    manifest: &BaseImageManifest,
) -> Result<(), RuntimeError> {
    let mut remaining = manifest.image_bytes();
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    while remaining > 0 {
        let requested =
            usize::try_from(remaining.min(buffer.len() as u64)).map_err(RuntimeError::manifest)?;
        let read = source
            .read(&mut buffer[..requested])
            .map_err(RuntimeError::manifest)?;
        if read == 0 {
            return Err(RuntimeError::manifest("verified image ended while staging"));
        }
        destination
            .write_all(&buffer[..read])
            .map_err(RuntimeError::manifest)?;
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    if source
        .read(&mut buffer[..1])
        .map_err(RuntimeError::manifest)?
        != 0
        || format!("{:x}", digest.finalize()) != manifest.image_sha256()
    {
        return Err(RuntimeError::manifest(
            "verified image changed while it was staged",
        ));
    }
    Ok(())
}
