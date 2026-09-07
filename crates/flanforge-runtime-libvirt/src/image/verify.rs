use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{Read, Seek},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    process::Stdio,
    time::Duration,
};

use flanforge_libvirt_wire::{BaseImageManifest, MAX_BASE_IMAGE_MANIFEST_BYTES};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::RuntimeError;

const MAX_QEMU_INFO_BYTES: usize = 64 * 1_024;

pub(crate) struct VerifiedImage {
    pub(crate) manifest: BaseImageManifest,
    pub(crate) file: File,
}

impl std::fmt::Debug for VerifiedImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedImage")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

/// Verifies an image against its manifest.
///
/// `manifest_path` is optional: the Packer build writes one, and an operator
/// who built a qcow2 another way has nothing to write. When it is absent the
/// manifest is read off the file itself, so the verification below compares
/// the image against measurements of that same image — which checks that it
/// did not change under us, not that someone vouched for it.
pub(crate) async fn verify_image(
    image_path: &Path,
    manifest_path: Option<&Path>,
    qemu_img_path: &Path,
) -> Result<VerifiedImage, RuntimeError> {
    let manifest = match manifest_path {
        Some(path) => read_manifest(path).await?,
        None => derive_manifest(image_path, qemu_img_path).await?,
    };
    let image_path = image_path.to_path_buf();
    let hashing_path = image_path.clone();
    let expected_file = manifest.image_file().to_owned();
    let expected_bytes = manifest.image_bytes();
    let expected_sha256 = manifest.image_sha256().to_owned();
    let (mut file, identity) = tokio::task::spawn_blocking(move || {
        open_and_hash(
            &hashing_path,
            &expected_file,
            expected_bytes,
            &expected_sha256,
        )
    })
    .await
    .map_err(|_| RuntimeError::manifest("image verification task failed"))??;
    let current = std::fs::metadata(image_path.as_path()).map_err(RuntimeError::manifest)?;
    if identity != FileIdentity::from_metadata(&current) {
        return Err(RuntimeError::manifest(
            "image changed while it was being verified",
        ));
    }
    file.rewind().map_err(RuntimeError::manifest)?;
    Ok(VerifiedImage { manifest, file })
}

async fn read_manifest(path: &Path) -> Result<BaseImageManifest, RuntimeError> {
    let path = path.to_owned();
    let bytes = tokio::task::spawn_blocking(move || {
        crate::file::read_bounded_regular(
            &path,
            MAX_BASE_IMAGE_MANIFEST_BYTES as u64,
            "image manifest",
        )
    })
    .await
    .map_err(|_| RuntimeError::manifest("manifest read task failed"))??;
    let manifest = BaseImageManifest::parse(&bytes).map_err(RuntimeError::manifest)?;
    ensure_runtime_compatible(&manifest)?;
    Ok(manifest)
}

/// Only what this runtime actually cannot do is refused here.
///
/// qcow2 is required because every guest boots from a qcow2 overlay backed by
/// this image, and `x86_64` because the domain XML this runtime emits declares
/// `arch="x86_64" machine="q35"`. The distribution and whatever container
/// runtime the image ships are the operator's business: nothing in the guest
/// channel depends on either, and a profile that needs one says so itself.
/// Guest size is not checked here — a profile already refuses to run smaller
/// than the base it boots from.
pub(crate) fn ensure_runtime_compatible(manifest: &BaseImageManifest) -> Result<(), RuntimeError> {
    if manifest.image_format() != "qcow2"
        || Path::new(manifest.image_file()).extension() != Some(OsStr::new("qcow2"))
    {
        return Err(RuntimeError::manifest(
            "libvirt guests boot from a qcow2 overlay, so the base must be qcow2",
        ));
    }
    // An unstated architecture is not a wrong one; the image either boots on
    // the emitted domain or it does not, and that is the operator's to know.
    if manifest
        .guest_architecture()
        .is_some_and(|architecture| architecture != "x86_64")
    {
        return Err(RuntimeError::manifest(
            "the libvirt runtime emits x86_64/q35 domains only",
        ));
    }
    Ok(())
}

fn open_and_hash(
    path: &Path,
    expected_file: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(File, FileIdentity), RuntimeError> {
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_file) {
        return Err(RuntimeError::manifest(
            "manifest does not describe the selected image file",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(RuntimeError::manifest)?;
    let metadata = file.metadata().map_err(RuntimeError::manifest)?;
    if !metadata.file_type().is_file() || metadata.len() != expected_bytes {
        return Err(RuntimeError::manifest(
            "image byte length does not match its manifest",
        ));
    }
    let identity = FileIdentity::from_metadata(&metadata);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(RuntimeError::manifest)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("{:x}", digest.finalize()) != expected_sha256 {
        return Err(RuntimeError::manifest(
            "image digest does not match its manifest",
        ));
    }
    Ok((file, identity))
}

/// Reads what the file itself can answer, for an operator-supplied image.
async fn derive_manifest(
    image_path: &Path,
    qemu_img_path: &Path,
) -> Result<BaseImageManifest, RuntimeError> {
    let document = qemu_img_info(image_path, qemu_img_path).await?;
    let format = document
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| RuntimeError::manifest("qemu-img did not report an image format"))?
        .to_owned();
    let virtual_bytes = document
        .get("virtual-size")
        .and_then(Value::as_u64)
        .ok_or_else(|| RuntimeError::manifest("qemu-img did not report a virtual size"))?;
    let file = image_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| RuntimeError::manifest("image path has no file name"))?
        .to_owned();
    let hashing_path = image_path.to_path_buf();
    let (bytes, sha256) = tokio::task::spawn_blocking(move || measure(&hashing_path))
        .await
        .map_err(|_| RuntimeError::manifest("image measurement task failed"))??;
    let manifest = BaseImageManifest::from_image(file, format, sha256, bytes, virtual_bytes);
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    Ok(manifest)
}

/// Hashes an image and reports its size, for a manifest nobody wrote.
fn measure(path: &Path) -> Result<(u64, String), RuntimeError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(RuntimeError::manifest)?;
    let bytes = file.metadata().map_err(RuntimeError::manifest)?.size();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1_048_576];
    loop {
        let read = file.read(&mut buffer).map_err(RuntimeError::manifest)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

async fn qemu_img_info(image_path: &Path, qemu_img_path: &Path) -> Result<Value, RuntimeError> {
    let mut child = Command::new(qemu_img_path)
        .args(["info", "--output=json", "--"])
        .arg(image_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(RuntimeError::manifest)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RuntimeError::manifest("qemu-img stdout is unavailable"))?;
    let capture = async {
        let mut bytes = Vec::new();
        let mut limited = stdout.take((MAX_QEMU_INFO_BYTES + 1) as u64);
        let read = limited.read_to_end(&mut bytes);
        let (_, status) = tokio::try_join!(read, child.wait())?;
        Ok::<_, std::io::Error>((status, bytes))
    };
    let result = tokio::time::timeout(Duration::from_secs(30), capture).await;
    let (status, stdout) = match result {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(RuntimeError::manifest(error));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(RuntimeError::Deadline);
        }
    };
    if !status.success() || stdout.len() > MAX_QEMU_INFO_BYTES {
        return Err(RuntimeError::manifest("qemu-img inspection failed"));
    }
    serde_json::from_slice(&stdout).map_err(RuntimeError::manifest)
}

pub(crate) async fn inspect_qcow2(
    image_path: &Path,
    qemu_img_path: &Path,
    manifest: &BaseImageManifest,
) -> Result<(), RuntimeError> {
    let document = qemu_img_info(image_path, qemu_img_path).await?;
    let is_standalone = document
        .get("backing-filename")
        .is_none_or(|value| value.is_null() || value.as_str() == Some(""));
    if document.get("format").and_then(Value::as_str) != Some("qcow2")
        || document.get("virtual-size").and_then(Value::as_u64) != Some(manifest.virtual_bytes())
        || !is_standalone
    {
        return Err(RuntimeError::manifest(
            "image format, virtual size, or backing-file state disagrees with the manifest",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}
