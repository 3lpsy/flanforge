use std::path::{Path, PathBuf};

use flanforge_core::{Config, ImageReimport, VmName};
use flanforge_libvirt_wire::BaseImageManifest;
use flanforge_store::StateMutationLock;

use crate::{RuntimeError, context::backend, image::load_published, image::publication_path};

use super::run::import_base;

const MAX_MANIFEST_BYTES: u64 = 64 * 1_024;
/// The configured directory plus one level of versioned subdirectories.
const MAX_SCAN_DEPTH: usize = 2;

/// Scans `runtime.backend.image_import_dir` and imports every
/// `<name>.qcow2` + `<name>.manifest.json` pair not yet published. Runs at
/// startup under the daemon's own mutation lock, before the listener binds.
///
/// Every failure is a warning, never a refusal to start: a bad drop in the
/// import directory must not take the allocation service down with it.
pub async fn auto_import_bases(config: &Config, lock: &StateMutationLock) {
    let backend = match backend(config) {
        Ok(backend) => backend,
        Err(error) => {
            tracing::warn!(%error, "image auto-import skipped by configuration");
            return;
        }
    };
    let Some(import_dir) = backend.image_import_dir.clone() else {
        tracing::debug!("no image import directory configured");
        return;
    };
    let candidates = match collect_candidates(&import_dir, MAX_SCAN_DEPTH) {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(directory = %import_dir.display(), %error, "cannot scan the image import directory");
            return;
        }
    };
    tracing::info!(directory = %import_dir.display(), candidates = candidates.len(), "image import scan");
    for candidate in candidates {
        auto_import_one(
            config,
            lock,
            &backend.image_manifest_dir,
            backend.image_reimport,
            &candidate,
        )
        .await;
    }
}

#[derive(Debug)]
struct Candidate {
    name: VmName,
    image: PathBuf,
    manifest: PathBuf,
}

async fn auto_import_one(
    config: &Config,
    lock: &StateMutationLock,
    manifest_dir: &Path,
    reimport: ImageReimport,
    candidate: &Candidate,
) {
    let sha256 = match read_manifest_sha256(&candidate.manifest) {
        Ok(sha256) => sha256,
        Err(error) => {
            tracing::warn!(name = %candidate.name, %error, "skipping an unreadable import manifest");
            return;
        }
    };
    let mut replace = false;
    if publication_path(manifest_dir, &candidate.name).exists() {
        match load_published(manifest_dir, &candidate.name) {
            Ok(existing) if existing.manifest().image_sha256() == sha256 => {
                tracing::info!(name = %candidate.name, "base already published; skipping");
                return;
            }
            Ok(existing) => match reimport {
                ImageReimport::Supersede => {
                    tracing::info!(
                        name = %candidate.name,
                        superseded_volume = existing.volume_name(),
                        "superseding the published base; the previous volume remains until removed"
                    );
                    replace = true;
                }
                ImageReimport::Refuse => {
                    tracing::warn!(
                        name = %candidate.name,
                        "a different base is already published under this name; \
                         set runtime.backend.image_reimport = \"supersede\" to replace it"
                    );
                    return;
                }
            },
            Err(error) => {
                tracing::warn!(name = %candidate.name, %error, "cannot read the existing publication; skipping");
                return;
            }
        }
    }
    tracing::info!(name = %candidate.name, image = %candidate.image.display(), "auto-importing base image");
    match import_base(
        config,
        lock,
        &candidate.name,
        &candidate.image,
        Some(&candidate.manifest),
        replace,
    )
    .await
    {
        Ok(publication) => {
            tracing::info!(
                name = %candidate.name,
                volume = publication.volume_name(),
                "base image auto-imported"
            );
        }
        Err(error) => {
            tracing::warn!(name = %candidate.name, %error, "base image auto-import failed");
        }
    }
}

fn read_manifest_sha256(path: &Path) -> Result<String, RuntimeError> {
    let bytes = crate::file::read_bounded_regular(path, MAX_MANIFEST_BYTES, "import manifest")?;
    let manifest = BaseImageManifest::parse(&bytes).map_err(RuntimeError::manifest)?;
    Ok(manifest.image_sha256().to_owned())
}

/// Sorted for a deterministic import order across restarts.
fn collect_candidates(directory: &Path, depth: usize) -> std::io::Result<Vec<Candidate>> {
    let mut candidates = Vec::new();
    scan(directory, depth, &mut candidates)?;
    candidates.sort_by(|a, b| a.image.cmp(&b.image));
    Ok(candidates)
}

fn scan(directory: &Path, depth: usize, candidates: &mut Vec<Candidate>) -> std::io::Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            scan(&path, depth - 1, candidates)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".manifest.json") else {
            continue;
        };
        let image = path.with_file_name(format!("{stem}.qcow2"));
        if !image.is_file() {
            tracing::warn!(manifest = %path.display(), "import manifest has no sibling qcow2");
            continue;
        }
        match VmName::new(stem) {
            Ok(name) => candidates.push(Candidate {
                name,
                image,
                manifest: path,
            }),
            Err(error) => {
                tracing::warn!(manifest = %path.display(), %error, "import name is not a valid image name");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_pair_manifests_with_siblings_and_skip_orphans() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let versioned = root.path().join("base-1");
        std::fs::create_dir(&versioned).unwrap_or_else(|error| unreachable!("{error}"));
        for (dir, name) in [
            (root.path(), "top"),
            (versioned.as_path(), "flanforge-base"),
        ] {
            std::fs::write(dir.join(format!("{name}.qcow2")), b"qcow2")
                .unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(dir.join(format!("{name}.manifest.json")), b"{}")
                .unwrap_or_else(|error| unreachable!("{error}"));
        }
        // Orphan manifest, unpaired image, and an invalid name are all skipped.
        std::fs::write(root.path().join("orphan.manifest.json"), b"{}")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(root.path().join("unpaired.qcow2"), b"qcow2")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(root.path().join("-bad.manifest.json"), b"{}")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(root.path().join("-bad.qcow2"), b"qcow2")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let candidates = collect_candidates(root.path(), MAX_SCAN_DEPTH)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let names = candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["flanforge-base", "top"]);
    }

    #[test]
    fn the_scan_does_not_descend_past_the_versioned_level() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deep = root.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deep.join("deep.qcow2"), b"qcow2")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deep.join("deep.manifest.json"), b"{}")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let candidates = collect_candidates(root.path(), MAX_SCAN_DEPTH)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(candidates.is_empty());
    }
}
