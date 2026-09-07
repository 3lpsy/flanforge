use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolves an explicit config argument or the platform default location.
///
/// # Errors
///
/// Returns an error when home expansion is required but `HOME` is missing or
/// is not an absolute path.
pub fn selected_config_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    selected_config_path_from(
        override_path,
        std::env::var_os(flanforge_paths::CONFIG_PATH_ENV).map(PathBuf::from),
    )
}

fn selected_config_path_from(
    override_path: Option<PathBuf>,
    environment_path: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return expand_cli_home(&path);
    }
    if let Some(path) = environment_path {
        return expand_cli_home(&path);
    }
    flanforge_paths::default_config_path().context("cannot resolve the default configuration path")
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
        Some(path) => expand_cli_home(&path)?,
        None => selected.to_owned(),
    };
    flanforge_paths::ensure_absolute_normalized(&path)
        .with_context(|| format!("{} must be an absolute normalized path", path.display()))?;
    Ok(path)
}

fn expand_cli_home(path: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    flanforge_paths::expand_home(path, home.as_deref()).context("cannot expand configuration path")
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
            expand_cli_home(&relative).unwrap_or_else(|error| unreachable!("path: {error}")),
            relative
        );
    }

    #[test]
    fn cli_config_path_overrides_environment_and_environment_overrides_default() {
        let environment = PathBuf::from("/from/environment.toml");
        assert_eq!(
            selected_config_path_from(None, Some(environment.clone()))
                .unwrap_or_else(|error| unreachable!("path: {error}")),
            environment
        );
        assert_eq!(
            selected_config_path_from(
                Some(PathBuf::from("/from/cli.toml")),
                Some(PathBuf::from("/from/environment.toml")),
            )
            .unwrap_or_else(|error| unreachable!("path: {error}")),
            PathBuf::from("/from/cli.toml")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_to_the_system_configuration() {
        assert_eq!(
            flanforge_paths::default_config_path()
                .unwrap_or_else(|error| unreachable!("path: {error}")),
            PathBuf::from("/etc/flanforge/config.toml")
        );
    }
}
