use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use flanforge_core::{Config, ConfigError};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const MAX_CONFIG_BYTES: u64 = 1_048_576;
const MAX_EXPANDED_STRING_BYTES: usize = 65_536;

/// Commented starter document: the shipped example is the single source, so a
/// generated configuration cannot drift from it.
pub const STARTER_CONFIG: &str = include_str!("../../../config.example.toml");

/// Reads config, expands environment placeholders as string data, and resolves
/// service-user home paths.
///
/// # Errors
///
/// Returns an error for unsafe files, malformed TOML or placeholders, missing
/// required environment values, or unavailable home-directory expansion.
pub async fn load_config(path: &Path) -> Result<Config, ConfigLoadError> {
    ConfigDocument::open(path).await?.resolved()
}

pub struct ConfigDocument {
    path: PathBuf,
    raw: toml::Value,
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
    ///
    /// Returns an error when the path is unsafe, unreadable, oversized, or is
    /// not valid UTF-8 TOML.
    pub async fn open(path: &Path) -> Result<Self, ConfigLoadError> {
        let raw = read_document(path).await?;
        Ok(Self {
            path: path.to_owned(),
            raw,
        })
    }

    #[must_use]
    pub fn raw(&self) -> &toml::Value {
        &self.raw
    }

    /// Expands string placeholders and home-relative paths into typed config.
    ///
    /// # Errors
    ///
    /// Returns an error for missing environment variables, malformed
    /// placeholders, unavailable home expansion, or invalid typed TOML.
    pub fn resolved(&self) -> Result<Config, ConfigLoadError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        decode_config(
            self.raw.clone(),
            &|name| std::env::var(name).ok(),
            home.as_deref(),
        )
    }

    /// Validates and atomically persists a replacement raw TOML document.
    ///
    /// # Errors
    ///
    /// Returns an error when expansion or validation fails or when the
    /// replacement cannot be durably written.
    pub async fn save(&mut self, raw: toml::Value) -> Result<(), ConfigLoadError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let config = decode_config(
            raw.clone(),
            &|name| std::env::var(name).ok(),
            home.as_deref(),
        )?;
        config.ensure_valid().map_err(ConfigLoadError::Validation)?;
        write_document(&self.path, &raw).await?;
        self.raw = raw;
        Ok(())
    }
}

async fn read_document(path: &Path) -> Result<toml::Value, ConfigLoadError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(ConfigLoadError::Inspect)?;
    if !metadata.file_type().is_file() {
        return Err(ConfigLoadError::NotRegular);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigLoadError::TooLarge);
    }
    let bytes = tokio::fs::read(path).await.map_err(ConfigLoadError::Read)?;
    let text = std::str::from_utf8(&bytes).map_err(ConfigLoadError::Utf8)?;
    toml::from_str(text).map_err(ConfigLoadError::Toml)
}

fn decode_config(
    mut value: toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Result<Config, ConfigLoadError> {
    expand_value(&mut value, environment)?;
    let mut config = value.try_into::<Config>().map_err(ConfigLoadError::Toml)?;
    resolve_paths(&mut config, home)?;
    Ok(config)
}

async fn write_document(path: &Path, value: &toml::Value) -> Result<(), ConfigLoadError> {
    let text = toml::to_string_pretty(value).map_err(ConfigLoadError::Serialize)?;
    write_config_text(path, &text).await
}

/// Durably replaces a configuration file with owner-only TOML text.
///
/// # Errors
///
/// Returns an error when the replacement cannot be created, written, renamed,
/// or synced.
pub async fn write_config_text(path: &Path, text: &str) -> Result<(), ConfigLoadError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".flanforge-config-{}.tmp", Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .await
        .map_err(ConfigLoadError::Write)?;
    if let Err(error) = async {
        file.write_all(text.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ConfigLoadError::Write(error));
    }
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ConfigLoadError::Write(error));
    }
    let directory = parent.to_owned();
    tokio::task::spawn_blocking(move || File::open(directory)?.sync_all())
        .await
        .map_err(|_| ConfigLoadError::Sync)?
        .map_err(ConfigLoadError::Write)
}

fn expand_value(
    value: &mut toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigLoadError> {
    match value {
        toml::Value::String(value) => *value = expand_string(value, environment)?,
        toml::Value::Array(values) => {
            for value in values {
                expand_value(value, environment)?;
            }
        }
        toml::Value::Table(values) => {
            for (_, value) in values.iter_mut() {
                expand_value(value, environment)?;
            }
        }
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => {}
    }
    Ok(())
}

fn expand_string(
    input: &str,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<String, ConfigLoadError> {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let expression = &remaining[start + 2..];
        let end = expression
            .find('}')
            .ok_or(ConfigLoadError::InvalidPlaceholder)?;
        let placeholder = &expression[..end];
        let (name, default) = placeholder
            .split_once(":-")
            .map_or((placeholder, None), |(name, default)| (name, Some(default)));
        if !is_environment_name(name) {
            return Err(ConfigLoadError::InvalidPlaceholder);
        }
        let configured = environment(name);
        let replacement = match (configured, default) {
            (Some(value), Some(default)) if value.is_empty() => default.to_owned(),
            (Some(value), _) => value,
            (None, Some(default)) => default.to_owned(),
            (None, None) => return Err(ConfigLoadError::MissingEnvironment(name.to_owned())),
        };
        output.push_str(&replacement);
        if output.len() > MAX_EXPANDED_STRING_BYTES {
            return Err(ConfigLoadError::ExpansionTooLarge);
        }
        remaining = &expression[end + 1..];
    }
    output.push_str(remaining);
    if output.len() > MAX_EXPANDED_STRING_BYTES {
        return Err(ConfigLoadError::ExpansionTooLarge);
    }
    Ok(output)
}

fn is_environment_name(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn resolve_paths(config: &mut Config, home: Option<&Path>) -> Result<(), ConfigLoadError> {
    if let Some(path) = &mut config.logging.path {
        resolve_home(path, home)?;
    }
    resolve_home(&mut config.forgejo.api_token_file, home)?;
    resolve_home(&mut config.runtime.state_dir, home)?;
    resolve_home(&mut config.runtime.tart_path, home)?;
    resolve_home(&mut config.runtime.ssh_path, home)?;
    resolve_home(&mut config.runtime.scp_path, home)?;
    resolve_home(&mut config.runtime.forgejo_runner_host_path, home)?;
    if let Some(path) = &mut config.runtime.tart_home {
        resolve_home(path, home)?;
    }
    resolve_home(&mut config.guest.ssh_identity_file, home)?;
    if let Some(path) = &mut config.guest.ssh_known_hosts_file {
        resolve_home(path, home)?;
    }
    resolve_home(&mut config.guest.forgejo_runner_path, home)?;
    if let Some(path) = &mut config.tailscale.preauth_key_file {
        resolve_home(path, home)?;
    }
    Ok(())
}

fn resolve_home(path: &mut PathBuf, home: Option<&Path>) -> Result<(), ConfigLoadError> {
    let Some(value) = path.to_str() else {
        return Ok(());
    };
    let suffix = if value == "~" {
        Some("")
    } else {
        value.strip_prefix("~/")
    };
    if let Some(suffix) = suffix {
        let home = home.ok_or(ConfigLoadError::HomeUnavailable)?;
        *path = if suffix.is_empty() {
            home.to_owned()
        } else {
            home.join(suffix)
        };
    }
    Ok(())
}

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
    #[error("configuration cannot be serialized: {0}")]
    Serialize(toml::ser::Error),
    #[error("cannot write configuration: {0}")]
    Write(io::Error),
    #[error("configuration directory sync task failed")]
    Sync,
    #[error("configuration is invalid: {0}")]
    Validation(ConfigError),
    #[error("configuration contains an invalid environment placeholder")]
    InvalidPlaceholder,
    #[error("configuration requires environment variable {0}")]
    MissingEnvironment(String),
    #[error("expanded configuration string exceeds 64 KiB")]
    ExpansionTooLarge,
    #[error("HOME is required to resolve a tilde path")]
    HomeUnavailable,
}

#[cfg(test)]
mod tests;
