use std::io;

use flanforge_core::ConfigError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("cannot inspect configuration: {0}")]
    Inspect(io::Error),
    #[error("configuration must be a regular file")]
    NotRegular,
    #[error("configuration exceeds 1 MiB")]
    TooLarge,
    #[error("cannot read configuration: {0}")]
    Read(io::Error),
    #[error("configuration is not UTF-8: {0}")]
    Utf8(std::str::Utf8Error),
    #[error("configuration is not valid TOML: {0}")]
    Toml(toml::de::Error),
    #[error("configuration layout cannot be parsed: {0}")]
    TomlLayout(toml_edit::TomlError),
    #[error("configuration cannot be serialized: {0}")]
    Serialize(toml::ser::Error),
    #[error("cannot write configuration: {0}")]
    Write(io::Error),
    #[error("configuration directory sync task failed")]
    Sync,
    #[error("another configuration writer holds the lock")]
    LockUnavailable,
    #[error("the configuration directory is not writable")]
    LockUnwritable,
    #[error("configuration is invalid: {0}")]
    Validation(ConfigError),
    #[error("`{field}` has been removed; {migration}")]
    RetiredField {
        field: &'static str,
        migration: &'static str,
    },
    #[error("profile {profile}: `{field}` has been removed; {migration}")]
    RetiredProfileField {
        profile: String,
        field: &'static str,
        migration: &'static str,
    },
    #[error("configuration contains an invalid environment placeholder")]
    InvalidPlaceholder,
    #[error("configuration requires environment variable {0}")]
    MissingEnvironment(String),
    #[error("profile {profile} requires environment variable {variable}")]
    UnresolvedProfilePlaceholder { profile: String, variable: String },
    #[error("expanded configuration string exceeds 64 KiB")]
    ExpansionTooLarge,
    #[error("HOME is required to resolve a tilde path")]
    HomeUnavailable,
    #[error("configuration override key is unsupported")]
    InvalidOverrideKey,
    #[error("configuration override value is invalid for {0}")]
    InvalidOverrideValue(String),
    #[error("configuration override cannot traverse a non-table value")]
    InvalidOverrideShape,
    #[error("native service installation cannot use ambient FLANFORGE__ overrides")]
    AmbientServiceOverrides,
}
