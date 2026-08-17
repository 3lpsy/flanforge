use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_config::ConfigDocument;
use flanforge_core::Profile;
use toml::Value;

use crate::cli::{ProfileCommand, ProfileCreateArgs, ProfileGetArgs, ProfileSetArgs};

use crate::commands::config::{emit, profile_value, selected_profile};

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
    let mut document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let mut raw = document.raw().clone();
    let profiles = profiles_mut(&mut raw)?;
    let name = arguments.profile.to_string();
    if profiles.contains_key(&name) {
        bail!("profile {name} already exists");
    }
    let profile = Profile {
        repository: arguments.repository,
        template: arguments.template,
        runner_label: arguments.runner_label,
        job_name: arguments.job_name,
        allowed_workflows: arguments.allowed_workflows.into_iter().collect(),
        allowed_events: arguments.allowed_events.into_iter().collect(),
        allowed_refs: arguments.allowed_refs.into_iter().collect(),
        allowed_ref_prefixes: arguments.allowed_ref_prefixes.into_iter().collect(),
        require_protected_ref: arguments.require_protected_ref,
        network: arguments.network.into(),
        cpu_count: arguments.cpu_count,
        memory_mb: arguments.memory_mb,
        boot_timeout_seconds: arguments.boot_timeout_seconds,
        idle_timeout_seconds: arguments.idle_timeout_seconds,
        job_timeout_seconds: arguments.job_timeout_seconds,
        cleanup_timeout_seconds: arguments.cleanup_timeout_seconds,
        warm_template: arguments.warm_template,
        regeneration_workflow: arguments.regeneration_workflow,
        reap: arguments.reap,
    };
    profiles.insert(
        name.clone(),
        Value::try_from(profile).context("cannot serialize profile")?,
    );
    document.save(raw).await.context("cannot save profile")?;
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
    let mut document = ConfigDocument::open(path)
        .await
        .with_context(|| format!("cannot open configuration {}", path.display()))?;
    let mut raw = document.raw().clone();
    let name = arguments.profile.as_str();
    let key = normalize_key(&arguments.key);
    let profile = profiles_mut(&mut raw)?
        .get_mut(name)
        .and_then(Value::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("profile {name} does not exist"))?;
    profile.insert(key.clone(), parse_value(&key, &arguments.value)?);
    document.save(raw).await.context("cannot save profile")?;
    tracing::info!(profile = %name, %key, "profile setting updated");
    Ok(())
}

fn profiles_mut(raw: &mut Value) -> Result<&mut toml::Table> {
    raw.get_mut("profiles")
        .and_then(Value::as_table_mut)
        .context("configuration has no profiles table")
}
