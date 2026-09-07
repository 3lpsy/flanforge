use std::{
    fs::DirBuilder,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use flanforge_core::ProfileName;
use flanforge_libvirt_wire::{MAX_PUBLISHED_WARM_BYTES, PublishedWarm};

use crate::{
    RuntimeError,
    file::{ensure_removed, ensure_replaced, write_private_temporary},
};

const SUFFIX: &str = ".warm.json";
/// One file per profile plus a bounded number of quarantined siblings.
const MAX_ENTRIES: usize = 4_096;

#[must_use]
pub(crate) fn directory(state_dir: &Path) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir).warm
}

#[must_use]
pub(crate) fn path(state_dir: &Path, profile: &ProfileName) -> PathBuf {
    directory(state_dir).join(format!("{profile}{SUFFIX}"))
}

/// Reads one profile's pointer, fail-closed exactly as the warm image store
/// is: anything that cannot be trusted is quarantined and read as absent, so
/// a bad file costs a cold boot rather than wedging the daemon.
pub(crate) fn load(
    state_dir: &Path,
    profile: &ProfileName,
) -> Result<Option<PublishedWarm>, RuntimeError> {
    let path = path(state_dir, profile);
    let Some(bytes) = crate::file::read_private_bounded_regular_optional(
        &path,
        MAX_PUBLISHED_WARM_BYTES as u64,
        "published warm pointer",
    )?
    else {
        return Ok(None);
    };
    match PublishedWarm::parse(&bytes) {
        Ok(document) if document.profile() == profile.as_str() => Ok(Some(document)),
        Ok(_) => {
            tracing::error!(%profile, "warm pointer names another profile; quarantining");
            quarantine(&path);
            Ok(None)
        }
        Err(error) => {
            tracing::error!(%profile, %error, "warm pointer is unreadable; quarantining");
            quarantine(&path);
            Ok(None)
        }
    }
}

/// Every profile's pointer, skipping anything the filename does not bind.
pub(crate) fn load_all(state_dir: &Path) -> Result<Vec<PublishedWarm>, RuntimeError> {
    let directory = directory(state_dir);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    .map_err(RuntimeError::manifest)?;
    if entries.len() > MAX_ENTRIES {
        return Err(RuntimeError::manifest(
            "warm pointer state has too many entries",
        ));
    }
    let mut documents = Vec::new();
    for entry in entries {
        let Some(profile) = profile_from_file_name(&entry.file_name()) else {
            continue;
        };
        if let Some(document) = load(state_dir, &profile)? {
            documents.push(document);
        }
    }
    Ok(documents)
}

/// Replaces one profile's pointer atomically. Promotion is exactly this write.
pub(crate) fn ensure_published(
    state_dir: &Path,
    document: &PublishedWarm,
) -> Result<(), RuntimeError> {
    let profile = ProfileName::new(document.profile()).map_err(RuntimeError::manifest)?;
    let bytes = document.encode().map_err(RuntimeError::manifest)?;
    let directory = directory(state_dir);
    ensure_private_directory(&directory)?;
    let temporary = write_private_temporary(&directory, "warm-pointer", &bytes)?;
    ensure_replaced(&temporary, &path(state_dir, &profile))
}

pub(crate) fn remove(state_dir: &Path, profile: &ProfileName) -> Result<(), RuntimeError> {
    ensure_removed(&path(state_dir, profile))
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
            "warm state path is not a private directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700).recursive(true);
            builder.create(path).map_err(RuntimeError::manifest)
        }
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

/// A filename that does not round-trip through `ProfileName` names no profile.
fn profile_from_file_name(file_name: &std::ffi::OsStr) -> Option<ProfileName> {
    let file_name = file_name.to_str()?;
    let stem = file_name.strip_suffix(SUFFIX)?;
    let profile = ProfileName::new(stem).ok()?;
    (format!("{profile}{SUFFIX}") == file_name).then_some(profile)
}

fn quarantine(path: &Path) {
    let quarantined = path.with_extension("json.corrupt");
    if let Err(error) = std::fs::rename(path, &quarantined) {
        tracing::error!(%error, "cannot quarantine an unreadable warm pointer");
    }
}
