use std::path::Path;

use anyhow::{Context, Result};
use flanforge_config::ConfigDocument;
use flanforge_core::Profile;
use toml::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use flanforge_cli::{ProfileCommand, ProfileCreateArgs, ProfileGetArgs, ProfileSetArgs};

use crate::config::{emit, profile_value, selected_profile};

use super::parse::{normalize_key, parse_value};

/// Executes a typed profile inspection or mutation command.
///
/// # Errors
///
/// Returns an error when the document or profile is missing, an input value is
/// invalid, or the validated document cannot be atomically saved.
pub async fn run_profile_command(path: &Path, command: ProfileCommand) -> Result<()> {
    match command {
        ProfileCommand::Create(arguments) => create(path, *arguments).await,
        ProfileCommand::Get(arguments) => get(path, &arguments).await,
        ProfileCommand::Set(arguments) => set(path, &arguments).await,
    }
}

async fn create(path: &Path, arguments: ProfileCreateArgs) -> Result<()> {
    // Held across open→edit→save, so a daemon-side edit cannot interleave.
    let _write_lock = flanforge_config::ConfigWriteLock::acquire(path)
        .await
        .context("another configuration writer holds the lock")?;
    let mut document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let mut formatted = document.formatted().clone();
    let name = arguments.profile.to_string();
    let profile = Profile {
        repository: arguments.repository,
        template: arguments.template,
        runner_label: arguments.runner_label,
        job_name: arguments.job_name,
        allowed_workflows: arguments.allowed_workflows.into_iter().collect(),
        allowed_events: arguments.allowed_events.into_iter().collect(),
        allowed_refs: arguments.allowed_refs.into_iter().collect(),
        require_protected_ref: arguments.require_protected_ref,
        network: arguments.network.into(),
        cpu_count: arguments.cpu_count,
        memory_mb: arguments.memory_mb,
        storage_mb: arguments.storage_mb,
        boot_timeout_seconds: arguments.boot_timeout_seconds,
        idle_timeout_seconds: arguments.idle_timeout_seconds,
        job_timeout_seconds: arguments.job_timeout_seconds,
        cleanup_timeout_seconds: arguments.cleanup_timeout_seconds,
        warm_template: arguments.warm_template,
        regeneration_workflow: arguments.regeneration_workflow,
        // `profile create` writes no hot table: hot is opted into by editing
        // the profile, never by a default the command chose.
        hot: None,
        reap: arguments.reap,
    };
    flanforge_config::insert_profile(&mut formatted, &name, &profile)?;
    document
        .save(formatted)
        .await
        .context("cannot save profile")?;
    tracing::info!(profile = %name, "profile created");
    Ok(())
}

async fn get(path: &Path, arguments: &ProfileGetArgs) -> Result<()> {
    let document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let name = arguments.profile.as_str();
    let raw = document.raw();
    if let Some(key) = &arguments.key {
        let key = normalize_key(key);
        let value = profile_value(raw, name)?
            .as_table()
            .and_then(|profile| profile.get(&key))
            .ok_or_else(|| anyhow::anyhow!("profile {name} has no key {key}"))?;
        let mut table = toml::Table::new();
        table.insert(key, value.clone());
        emit(&Value::Table(table))
    } else {
        emit(&selected_profile(raw, name)?)?;
        print!("{}", warm_status(profile_value(raw, name)?));
        Ok(())
    }
}

/// Configuration-level warm status only; whether the image exists, is current,
/// and is claimed is daemon state, not document state.
pub(super) fn warm_status(profile: &Value) -> String {
    let text = |key: &str| profile.get(key).and_then(Value::as_str).map(str::to_owned);
    let Some(warm) = text("warm_template") else {
        return "# warm: not declared; every job boots the profile template\n".to_owned();
    };
    let workflow = text("regeneration_workflow").unwrap_or_default();
    format!(
        "# warm: declared as {warm}, produced only by {workflow}\n\
         # warm: configured, not observed; ask the daemon what a job will clone\n"
    )
}

async fn set(path: &Path, arguments: &ProfileSetArgs) -> Result<()> {
    let _write_lock = flanforge_config::ConfigWriteLock::acquire(path)
        .await
        .context("another configuration writer holds the lock")?;
    let mut document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let mut formatted = document.formatted().clone();
    let name = arguments.profile.as_str();
    let key = normalize_key(&arguments.key);
    let profile = profiles_mut(&mut formatted)?
        .as_table_like_mut()
        .and_then(|profiles| profiles.get_mut(name))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| anyhow::anyhow!("profile {name} does not exist"))?;
    set_value(profile, &key, parse_value(&key, &arguments.value)?);
    document
        .save(formatted)
        .await
        .context("cannot save profile")?;
    tracing::info!(profile = %name, %key, "profile setting updated");
    Ok(())
}

fn profiles_mut(document: &mut DocumentMut) -> Result<&mut Item> {
    document
        .get_mut("profiles")
        .filter(|profiles| profiles.as_table_like().is_some())
        .context("configuration has no profiles table")
}

/// Replaces the value in place: inserting over an existing key would reformat
/// it, dropping the comment and spacing the operator wrote around it.
fn set_value(profile: &mut dyn TableLike, key: &str, mut value: toml_edit::Value) {
    if let Some(existing) = profile.get(key).and_then(Item::as_value) {
        *value.decor_mut() = existing.decor().clone();
    }
    if let Some(existing) = profile.get_mut(key) {
        *existing = Item::Value(value);
    } else {
        profile.insert(key, Item::Value(value));
    }
}
