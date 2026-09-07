use std::path::{Path, PathBuf};

use crate::PathError;

/// Reads and validates the service account's home directory.
///
/// # Errors
/// Returns an error when `HOME` is absent or not absolute.
pub fn service_home() -> Result<PathBuf, PathError> {
    let home = std::env::var_os("HOME").ok_or(PathError::HomeUnavailable)?;
    validated_home(PathBuf::from(home))
}

/// Expands only `~` and `~/` against an explicit home directory.
///
/// # Errors
/// Returns an error when expansion needs an absent or relative home.
pub fn expand_home(path: &Path, home: Option<&Path>) -> Result<PathBuf, PathError> {
    let Some(value) = path.to_str() else {
        return Ok(path.to_owned());
    };
    let suffix = if value == "~" {
        Some("")
    } else {
        value.strip_prefix("~/")
    };
    let Some(suffix) = suffix else {
        return Ok(path.to_owned());
    };
    let home = home.ok_or(PathError::HomeUnavailable)?;
    let home = validated_home(home.to_owned())?;
    Ok(if suffix.is_empty() {
        home
    } else {
        home.join(suffix)
    })
}

fn validated_home(home: PathBuf) -> Result<PathBuf, PathError> {
    if home.is_absolute() {
        Ok(home)
    } else {
        Err(PathError::HomeNotAbsolute)
    }
}
