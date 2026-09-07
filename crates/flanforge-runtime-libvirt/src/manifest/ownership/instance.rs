use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use uuid::Uuid;

use crate::{RuntimeError, file::read_private_bounded_regular_optional};

use super::super::paths;

const MAX_INSTANCE_BYTES: u64 = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ServiceInstance(Uuid);

impl ServiceInstance {
    pub(crate) fn load_or_create(state_dir: &Path) -> Result<Self, RuntimeError> {
        let state = flanforge_paths::libvirt_state_paths(state_dir);
        ensure_private_directory(&state.root)?;
        ensure_private_directory(&state.allocations)?;
        remove_stale_temporaries(&state.root)?;
        let path = paths::service_instance(state_dir);
        loop {
            if let Some(bytes) = read_private_bounded_regular_optional(
                &path,
                MAX_INSTANCE_BYTES,
                "service instance ID",
            )? {
                return parse_instance(&bytes);
            }

            let identity = Uuid::new_v4();
            let temporary = state.root.join(format!(".service-instance-{identity}.tmp"));
            let result = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&temporary)?;
                writeln!(file, "{identity}")?;
                file.sync_all()?;
                std::fs::hard_link(&temporary, &path)
            })();
            match result {
                Ok(()) => {
                    std::fs::remove_file(&temporary).map_err(RuntimeError::manifest)?;
                    sync_directory(&state.root)?;
                    return Ok(Self(identity));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_regular_if_present(&temporary)?;
                }
                Err(error) => {
                    let _ = remove_regular_if_present(&temporary);
                    return Err(RuntimeError::manifest(error));
                }
            }
        }
    }

    pub(crate) const fn id(&self) -> Uuid {
        self.0
    }
}

// The octal mask names the group and other bits a reader checks for; a
// trailing-zero count does not.
#[allow(clippy::verbose_bit_mask)]
fn ensure_private_directory(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_dir() && metadata.permissions().mode() & 0o077 == 0 =>
        {
            Ok(())
        }
        Ok(_) => Err(RuntimeError::manifest(
            "libvirt state path is not a private directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700).recursive(true);
            builder.create(path).map_err(RuntimeError::manifest)
        }
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn remove_stale_temporaries(directory: &Path) -> Result<(), RuntimeError> {
    let entries = std::fs::read_dir(directory)
        .map_err(RuntimeError::manifest)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RuntimeError::manifest)?;
    if entries.len() > 1_024 {
        return Err(RuntimeError::manifest("libvirt state has too many entries"));
    }
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(RuntimeError::manifest(
                "libvirt state filename is not UTF-8",
            ));
        };
        if name
            .strip_prefix(".service-instance-")
            .and_then(|value| value.strip_suffix(".tmp"))
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
        {
            remove_regular_if_present(&entry.path())?;
        }
    }
    sync_directory(directory)
}

fn remove_regular_if_present(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)
        }
        Ok(_) => Err(RuntimeError::manifest(
            "refusing to remove non-file service instance state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn sync_directory(directory: &Path) -> Result<(), RuntimeError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(RuntimeError::manifest)
}

fn parse_instance(bytes: &[u8]) -> Result<ServiceInstance, RuntimeError> {
    let value = std::str::from_utf8(bytes).map_err(RuntimeError::manifest)?;
    Uuid::parse_str(value.trim())
        .map(ServiceInstance)
        .map_err(RuntimeError::manifest)
}
