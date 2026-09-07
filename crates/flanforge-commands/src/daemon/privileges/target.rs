use std::{
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use flanforge_service_launchctl::launchctl_paths;

/// The binary the gates are established for, and how to reach it.
///
/// All three gates key on a binary's identity at a path, and launchd runs the
/// installed copy, so that path is the default subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) binary: PathBuf,
    /// Set when the probes must be performed by another executable, because a
    /// consent prompt is attributed to the binary doing the access.
    pub(super) probe_via: Option<PathBuf>,
    pub(super) note: Option<String>,
}

pub(super) fn resolve() -> Result<Target> {
    let paths = launchctl_paths()?;
    let current = std::env::current_exe().context("cannot locate current executable")?;
    let is_present = match std::fs::metadata(&paths.binary) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => anyhow::bail!(
            "installed service path {} is not a file",
            paths.binary.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot inspect installed service path {}",
                    paths.binary.display()
                )
            });
        }
    };
    Ok(decide(&paths.binary, is_present, &current))
}

pub(super) fn decide(installed: &Path, is_installed_present: bool, current: &Path) -> Target {
    if !is_installed_present {
        return Target {
            binary: current.to_owned(),
            probe_via: None,
            note: Some(format!(
                "{} is absent, so this run authorizes the running executable; \
                 launchd runs the installed path, so run `daemon install` and then this command again",
                installed.display()
            )),
        };
    }
    if installed == current {
        return Target {
            binary: installed.to_owned(),
            probe_via: None,
            note: None,
        };
    }
    Target {
        binary: installed.to_owned(),
        probe_via: Some(installed.to_owned()),
        note: Some(format!(
            "running {}, which is not the installed binary; the gates above are established for the installed one",
            current.display()
        )),
    }
}
