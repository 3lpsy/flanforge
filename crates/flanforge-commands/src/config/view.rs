use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_config::ConfigDocument;
use toml::{Table, Value};

use flanforge_cli::ConfigViewArgs;

pub(super) async fn view(path: &Path, arguments: &ConfigViewArgs) -> Result<()> {
    emit(&viewed(path, arguments).await?)
}

/// Reads the document, resolves it when asked, and narrows the selection.
pub(super) async fn viewed(path: &Path, arguments: &ConfigViewArgs) -> Result<Value> {
    let document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let value = if arguments.resolved {
        resolved_view(path, &document)?
    } else {
        document.canonical_view()
    };
    select(&value, arguments)
}

/// Renders the effective configuration into the raw view's shape, so both
/// views share one selection path.
fn resolved_view(path: &Path, document: &ConfigDocument) -> Result<Value> {
    let config = document
        .resolved()
        .with_context(|| format!("cannot resolve configuration {}", path.display()))?;
    Value::try_from(config).context("cannot render the resolved configuration")
}

fn select(raw: &Value, arguments: &ConfigViewArgs) -> Result<Value> {
    if arguments.logging {
        selected_section(raw, "logging")
    } else if arguments.server {
        selected_section(raw, "server")
    } else if arguments.oidc {
        selected_section(raw, "oidc")
    } else if arguments.forgejo {
        selected_section(raw, "forgejo")
    } else if arguments.runtime {
        selected_section(raw, "runtime")
    } else if arguments.guest {
        selected_section(raw, "guest")
    } else if arguments.tailscale {
        selected_section(raw, "tailscale")
    } else if arguments.profiles {
        selected_section(raw, "profiles")
    } else if let Some(profile) = &arguments.profile {
        selected_profile(raw, profile.as_str())
    } else {
        Ok(raw.clone())
    }
}

pub(crate) fn selected_profile(raw: &Value, name: &str) -> Result<Value> {
    let profile = profile_value(raw, name)?.clone();
    let mut profiles = Table::new();
    profiles.insert(name.to_owned(), profile);
    let mut root = Table::new();
    root.insert("profiles".to_owned(), Value::Table(profiles));
    Ok(Value::Table(root))
}

pub(crate) fn profile_value<'a>(raw: &'a Value, name: &str) -> Result<&'a Value> {
    raw.get("profiles")
        .and_then(Value::as_table)
        .and_then(|profiles| profiles.get(name))
        .ok_or_else(|| anyhow::anyhow!("profile {name} does not exist"))
}

pub(crate) fn emit(value: &Value) -> Result<()> {
    let rendered = toml::to_string_pretty(value).context("cannot render configuration")?;
    print!("{rendered}");
    Ok(())
}

fn selected_section(raw: &Value, name: &str) -> Result<Value> {
    let Some(value) = raw.get(name) else {
        bail!("configuration section {name} does not exist");
    };
    let mut root = Table::new();
    root.insert(name.to_owned(), value.clone());
    Ok(Value::Table(root))
}
