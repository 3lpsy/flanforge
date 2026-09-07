use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use validator::Validate;

/// One supported key of the configuration schema. Profile keys carry a
/// `profiles.*` pattern the UI expands per profile name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfigFieldView {
    pub path: String,
    /// The overlay schema's kind name, e.g. `bool` or `optional_string`.
    pub kind: String,
    /// Whether PATCH accepts this key; everything else is view only.
    pub editable: bool,
}

/// The whole configuration for the schema-driven view: the document exactly
/// as written — `${VAR}` placeholders stay placeholders — plus the running
/// values for showing what an unset key resolves to.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConfigView {
    pub document: serde_json::Value,
    /// The resolved running configuration; paths only, never secret contents.
    pub effective: serde_json::Value,
    /// Every supported key, so unset ones still render and can be added.
    pub schema: Vec<ConfigFieldView>,
    /// Optimistic concurrency token; send it back with the update.
    pub version: String,
    pub restart_pending: Vec<String>,
    /// Reload count since the daemon started.
    pub generation: u64,
    /// False when the file is managed outside the daemon (a read-only
    /// mount); edits are refused and the UI shows a read-only view.
    #[serde(default = "default_writable")]
    pub writable: bool,
}

fn default_writable() -> bool {
    true
}

/// Typed changes to allowlisted keys. `null` clears an optional key.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct ConfigUpdate {
    #[validate(length(min = 64, max = 64))]
    pub version: String,
    #[validate(custom(function = "validate_changes"))]
    pub changes: BTreeMap<String, serde_json::Value>,
}

fn validate_changes(
    changes: &BTreeMap<String, serde_json::Value>,
) -> Result<(), validator::ValidationError> {
    if changes.is_empty()
        || changes.len() > 64
        || changes.keys().any(|key| key.is_empty() || key.len() > 200)
    {
        return Err(validator::ValidationError::new("changes"));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfigUpdateResult {
    /// The version after the write; the next edit builds on it.
    pub version: String,
    pub restart_pending: Vec<String>,
    /// False when the daemon's reload did not confirm within the wait; the
    /// watcher applies it within seconds either way.
    pub reloaded: bool,
}

/// A whole new profile, shaped like its TOML table.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct ProfileCreate {
    #[validate(length(min = 64, max = 64))]
    pub version: String,
    #[validate(custom(function = "validate_profile_object"))]
    pub profile: serde_json::Value,
}

fn validate_profile_object(profile: &serde_json::Value) -> Result<(), validator::ValidationError> {
    if profile.is_object() {
        Ok(())
    } else {
        Err(validator::ValidationError::new("profile"))
    }
}

/// Removes a profile; the version token guards against racing edits.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct ProfileDelete {
    #[validate(length(min = 64, max = 64))]
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileDeleteResult {
    pub version: String,
    pub restart_pending: Vec<String>,
    pub reloaded: bool,
    /// Hot machines that were serving the profile when it was removed; the
    /// reload drains them.
    pub hot_machines: Vec<String>,
}
