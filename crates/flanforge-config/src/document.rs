use std::path::{Path, PathBuf};

use flanforge_core::Config;
use toml_edit::DocumentMut;

use crate::{
    ConfigLoadError, ConfigOverrides,
    decode::{canonicalize_runtime, decode_with_process_environment},
    deferred::ensure_profiles_valid,
    storage::{read_document_text, write_document},
};

/// Reads, expands, and resolves the selected configuration.
///
/// # Errors
/// Returns an error for unsafe files, malformed values, or invalid overrides.
pub async fn load_config(path: &Path) -> Result<Config, ConfigLoadError> {
    ConfigDocument::open(path).await?.resolved()
}

/// Resolves config with explicit CLI overrides as the final layer.
///
/// # Errors
/// Returns the same errors as [`load_config`], plus invalid override errors.
pub async fn load_config_with_overrides(
    path: &Path,
    overrides: &ConfigOverrides,
) -> Result<Config, ConfigLoadError> {
    ConfigDocument::open(path)
        .await?
        .resolved_with_overrides(overrides)
}

pub struct ConfigDocument {
    path: PathBuf,
    raw: toml::Value,
    formatted: DocumentMut,
}

impl std::fmt::Debug for ConfigDocument {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigDocument")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ConfigDocument {
    /// Opens a size-bounded regular TOML file without expanding its strings.
    ///
    /// # Errors
    /// Returns an error when the file is unsafe, unreadable, or malformed.
    pub async fn open(path: &Path) -> Result<Self, ConfigLoadError> {
        let text = read_document_text(path).await?;
        let (raw, formatted) = parse(&text)?;
        Ok(Self {
            path: path.to_owned(),
            raw,
            formatted,
        })
    }

    #[must_use]
    pub fn raw(&self) -> &toml::Value {
        &self.raw
    }

    /// Returns the file as written, with its comments, key order, and spacing.
    #[must_use]
    pub fn formatted(&self) -> &DocumentMut {
        &self.formatted
    }

    /// Returns the canonical structure without resolving environment values.
    #[must_use]
    pub fn canonical_view(&self) -> toml::Value {
        let mut value = self.raw.clone();
        canonicalize_runtime(&mut value);
        value
    }

    /// Resolves defaults, TOML, and process environment.
    ///
    /// # Errors
    /// Returns an error for malformed values, overrides, or paths.
    pub fn resolved(&self) -> Result<Config, ConfigLoadError> {
        self.resolved_with_overrides(&ConfigOverrides::default())
    }

    /// Resolves defaults, TOML, environment, then explicit CLI overrides.
    ///
    /// # Errors
    /// Returns an error for malformed values, overrides, or paths.
    pub fn resolved_with_overrides(
        &self,
        overrides: &ConfigOverrides,
    ) -> Result<Config, ConfigLoadError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        decode_with_process_environment(self.raw.clone(), overrides, home.as_deref())
    }

    /// Validates and atomically persists an edited format-preserving document.
    ///
    /// Validation reads back exactly the bytes about to be written, and uses
    /// the same environment layer as normal loading. A `${KEY}` this
    /// environment cannot resolve is not a verdict on the edit: the profiles
    /// are then validated on their own and the deployment is left to the
    /// daemon, which resolves it when it loads the file.
    ///
    /// # Errors
    /// Returns an error when validation or durable replacement fails.
    pub async fn save(&mut self, formatted: DocumentMut) -> Result<(), ConfigLoadError> {
        let raw = parse_raw(&formatted.to_string())?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let environment = |name: &str| std::env::var(name).ok();
        match decode_with_process_environment(
            raw.clone(),
            &ConfigOverrides::default(),
            home.as_deref(),
        ) {
            Ok(config) => config.ensure_valid().map_err(ConfigLoadError::Validation)?,
            Err(ConfigLoadError::MissingEnvironment(variable)) => {
                ensure_profiles_valid(&raw, &environment)?;
                tracing::warn!(
                    variable = %variable,
                    "environment variable is unset here, so only the profiles were validated; the daemon validates the whole document when it loads it"
                );
            }
            Err(error) => return Err(error),
        }
        write_document(&self.path, &formatted).await?;
        self.raw = raw;
        self.formatted = formatted;
        Ok(())
    }
}

/// Both views come from the same text: one to resolve, one to rewrite.
fn parse(text: &str) -> Result<(toml::Value, DocumentMut), ConfigLoadError> {
    let raw = parse_raw(text)?;
    let formatted = text
        .parse::<DocumentMut>()
        .map_err(ConfigLoadError::TomlLayout)?;
    Ok((raw, formatted))
}

fn parse_raw(text: &str) -> Result<toml::Value, ConfigLoadError> {
    toml::from_str(text).map_err(ConfigLoadError::Toml)
}
