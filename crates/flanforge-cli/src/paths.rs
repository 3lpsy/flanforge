use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Resolves an explicit config argument or the platform default location.
///
/// # Errors
///
/// Returns an error when home expansion is required but `HOME` is missing or
/// is not an absolute path.
pub fn selected_config_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return expand_cli_home(path);
    }
    default_config_path()
}

/// Resolves the destination a generated document is written to, defaulting to
/// the already selected configuration path.
///
/// # Errors
///
/// Returns an error when home expansion fails or the destination is not a
/// plain absolute path.
pub(crate) fn generated_config_path(
    override_path: Option<PathBuf>,
    selected: &Path,
) -> Result<PathBuf> {
    let path = match override_path {
        Some(path) => expand_cli_home(path)?,
        None => selected.to_owned(),
    };
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "{} must be an absolute path without parent traversal",
            path.display()
        );
    }
    Ok(path)
}

pub(crate) fn default_config_path() -> Result<PathBuf> {
    let home = service_home()?;
    #[cfg(target_os = "macos")]
    {
        Ok(home.join("Library/Application Support/flanforge/config.toml"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            let xdg = PathBuf::from(xdg);
            if xdg.is_absolute() {
                return Ok(xdg.join("flanforge/config.toml"));
            }
        }
        Ok(home.join(".config/flanforge/config.toml"))
    }
}

pub(crate) fn service_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is required")?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        bail!("HOME must be an absolute path");
    }
    Ok(home)
}

fn expand_cli_home(path: PathBuf) -> Result<PathBuf> {
    let Some(value) = path.to_str() else {
        return Ok(path);
    };
    let suffix = if value == "~" {
        Some("")
    } else {
        value.strip_prefix("~/")
    };
    let Some(suffix) = suffix else {
        return Ok(path);
    };
    let home = service_home()?;
    Ok(if suffix.is_empty() {
        home
    } else {
        home.join(Path::new(suffix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_paths_reject_relative_and_other_user_forms() {
        let selected = PathBuf::from("/private/flanforge/config.toml");
        assert_eq!(
            generated_config_path(None, &selected)
                .unwrap_or_else(|error| unreachable!("path: {error}")),
            selected
        );
        for rejected in [
            "config.toml",
            "~someone/config.toml",
            "/private/../config.toml",
        ] {
            assert!(
                generated_config_path(Some(PathBuf::from(rejected)), &selected).is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn explicit_non_tilde_paths_are_preserved() {
        let relative = PathBuf::from("config.toml");
        assert_eq!(
            expand_cli_home(relative.clone()).unwrap_or_else(|error| unreachable!("path: {error}")),
            relative
        );
    }
}
