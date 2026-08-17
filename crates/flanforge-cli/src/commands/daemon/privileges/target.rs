use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::super::paths::ServicePaths;

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
    let paths = ServicePaths::discover()?;
    let current = std::env::current_exe().context("cannot locate current executable")?;
    Ok(decide(&paths.binary, paths.binary.is_file(), &current))
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
