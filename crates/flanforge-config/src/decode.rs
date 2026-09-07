use std::path::{Path, PathBuf};

use flanforge_core::{Config, RuntimeBackendConfig};

use crate::{
    ConfigLoadError, ConfigOverrides,
    expand::expand_value,
    overlay::{ConfigResolver, process_environment_overrides},
    retired::{ensure_no_retired_fields, ensure_no_retired_profile_fields},
};

#[cfg(test)]
pub(crate) fn decode_config(
    value: toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Result<Config, ConfigLoadError> {
    decode_config_with_overrides(value, environment, &[], &[], home)
}

pub(crate) fn decode_config_with_overrides(
    value: toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
    environment_overrides: &[(String, String)],
    cli_overrides: &[(&str, &str)],
    home: Option<&Path>,
) -> Result<Config, ConfigLoadError> {
    let mut resolver = ConfigResolver::default();
    resolver.apply_config_file(value)?;
    resolver.apply_env(environment_overrides)?;
    resolver.apply_cli(cli_overrides)?;
    let mut value = resolver.finish();
    expand_value(&mut value, environment)?;
    if is_legacy_runtime_document(&value) {
        tracing::warn!(
            "a runtime table without [runtime.backend] is deprecated and defaults to Tart"
        );
    }
    ensure_no_retired_fields(&value)?;
    ensure_no_retired_profile_fields(&value)?;
    let mut config = value.try_into::<Config>().map_err(ConfigLoadError::Toml)?;
    resolve_paths(&mut config, home)?;
    Ok(config)
}

pub(crate) fn decode_with_process_environment(
    value: toml::Value,
    overrides: &ConfigOverrides,
    home: Option<&Path>,
) -> Result<Config, ConfigLoadError> {
    let environment_overrides = process_environment_overrides()?;
    let cli_overrides = overrides.pairs();
    decode_config_with_overrides(
        value,
        &|name| std::env::var(name).ok(),
        &environment_overrides,
        &cli_overrides,
        home,
    )
}

pub(crate) fn is_legacy_runtime_document(value: &toml::Value) -> bool {
    let Some(runtime) = value.get("runtime").and_then(toml::Value::as_table) else {
        return false;
    };
    !runtime.contains_key("backend")
        && ["tart_path", "tart_home", "forgejo_runner_host_path"]
            .iter()
            .any(|key| runtime.contains_key(*key))
}

pub(crate) fn canonicalize_runtime(value: &mut toml::Value) {
    let Some(runtime) = value.get_mut("runtime").and_then(toml::Value::as_table_mut) else {
        return;
    };
    if runtime.contains_key("backend") {
        return;
    }
    let has_legacy_tart = ["tart_path", "tart_home", "forgejo_runner_host_path"]
        .iter()
        .any(|key| runtime.contains_key(*key));
    let backend = if has_legacy_tart {
        canonical_tart_backend(runtime)
    } else {
        toml::Value::try_from(RuntimeBackendConfig::default())
            .unwrap_or_else(|_| unreachable!("runtime defaults serialize"))
    };
    runtime.insert("backend".to_owned(), backend);
}

fn canonical_tart_backend(runtime: &mut toml::Table) -> toml::Value {
    let mut backend = toml::Table::new();
    backend.insert("kind".to_owned(), toml::Value::String("tart".to_owned()));
    backend.insert(
        "path".to_owned(),
        runtime.remove("tart_path").unwrap_or_else(|| {
            toml::Value::String(
                flanforge_paths::default_tart_path()
                    .to_string_lossy()
                    .into_owned(),
            )
        }),
    );
    backend.insert(
        "runner_host_path".to_owned(),
        runtime
            .remove("forgejo_runner_host_path")
            .unwrap_or_else(|| {
                toml::Value::String(
                    flanforge_paths::default_runner_host_path()
                        .to_string_lossy()
                        .into_owned(),
                )
            }),
    );
    if let Some(home) = runtime.remove("tart_home") {
        backend.insert("home".to_owned(), home);
    }
    toml::Value::Table(backend)
}

fn resolve_paths(config: &mut Config, home: Option<&Path>) -> Result<(), ConfigLoadError> {
    if let Some(path) = &mut config.logging.path {
        resolve_home(path, home)?;
    }
    resolve_home(&mut config.forgejo.api_token_file, home)?;
    resolve_home(&mut config.runtime.state_dir, home)?;
    match &mut config.runtime.backend {
        RuntimeBackendConfig::Tart(tart) => {
            resolve_home(&mut tart.path, home)?;
            if let Some(path) = &mut tart.runner_host_path {
                resolve_home(path, home)?;
            }
            if let Some(path) = &mut tart.home {
                resolve_home(path, home)?;
            }
        }
        RuntimeBackendConfig::Libvirt(libvirt) => {
            resolve_home(&mut libvirt.image_manifest_dir, home)?;
            resolve_home(&mut libvirt.qemu_img_path, home)?;
            resolve_home(&mut libvirt.virsh_path, home)?;
        }
    }
    resolve_home(&mut config.runtime.ssh_path, home)?;
    resolve_home(&mut config.runtime.scp_path, home)?;
    if let Some(ssh) = &mut config.guest.ssh {
        resolve_home(&mut ssh.identity_file, home)?;
        if let Some(path) = &mut ssh.known_hosts_file {
            resolve_home(path, home)?;
        }
    }
    resolve_home(&mut config.guest.forgejo_runner_path, home)?;
    if let Some(path) = &mut config.tailscale.preauth_key_file {
        resolve_home(path, home)?;
    }
    Ok(())
}

pub(crate) fn resolve_home(path: &mut PathBuf, home: Option<&Path>) -> Result<(), ConfigLoadError> {
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
