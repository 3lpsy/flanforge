use std::{collections::BTreeSet, ffi::OsString};

use crate::ConfigLoadError;

pub(crate) const ENV_PREFIX: &str = "FLANFORGE__";

pub(crate) fn process_environment_overrides() -> Result<Vec<(String, String)>, ConfigLoadError> {
    collect_environment_overrides(std::env::vars_os())
}

pub(crate) fn collect_environment_overrides(
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<Vec<(String, String)>, ConfigLoadError> {
    let mut result = Vec::new();
    let mut keys = BTreeSet::new();
    for (name, value) in environment {
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(key) = environment_key(name) else {
            continue;
        };
        if key.is_empty() || !keys.insert(key.clone()) {
            return Err(ConfigLoadError::InvalidOverrideKey);
        }
        let value = value
            .into_string()
            .map_err(|_| ConfigLoadError::InvalidOverrideValue(key.clone()))?;
        result.push((key, value));
    }
    Ok(result)
}

pub(crate) fn environment_key(name: &str) -> Option<String> {
    let suffix = name.strip_prefix(ENV_PREFIX)?;
    Some(
        suffix
            .split("__")
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            .join("."),
    )
}
