use std::collections::BTreeSet;

use flanforge_utils::is_safe_git_ref;

use crate::{PREVIOUS_SUFFIX, RunnerLabel, VmName};

use super::{Config, ConfigError, Profile};

/// Longest reserved suffix a warm image name must still leave room for.
const RESERVED_SUFFIX_LEN: usize = PREVIOUS_SUFFIX.len();

pub(super) fn ensure_profiles_valid(config: &Config) -> Result<(), ConfigError> {
    let mut repositories = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for (name, profile) in &config.profiles {
        ensure_profile(name.as_str(), profile, config.runtime.vm_prefix.as_str())?;
        if !repositories.insert(profile.repository.clone())
            || !labels.insert(profile.runner_label.clone())
        {
            return Err(ConfigError::DuplicateProfileBinding);
        }
    }
    Ok(())
}

fn ensure_profile(name: &str, profile: &Profile, vm_prefix: &str) -> Result<(), ConfigError> {
    let invalid = |message: &str| ConfigError::InvalidProfile {
        profile: name.to_owned(),
        message: message.to_owned(),
    };
    if profile.template.as_str().starts_with(vm_prefix) {
        return Err(invalid("template cannot use the service-owned VM prefix"));
    }
    if profile.runner_label.as_str().len() + 1 + 36 > RunnerLabel::MAX_LEN {
        return Err(invalid(
            "runner label is too long for a per-allocation suffix",
        ));
    }
    let max_vm_name = vm_prefix.len() + name.len() + 1 + 20 + 1 + 10;
    if max_vm_name > VmName::MAX_LEN {
        return Err(invalid("generated VM name can exceed its maximum length"));
    }
    if profile.allowed_workflows.is_empty()
        || profile.allowed_events.is_empty()
        || (profile.allowed_refs.is_empty() && profile.allowed_ref_prefixes.is_empty())
    {
        return Err(invalid("authorization allowlists cannot be empty"));
    }
    if !profile
        .allowed_workflows
        .iter()
        .all(|value| is_safe_workflow(value))
    {
        return Err(invalid("workflow allowlist contains an unsafe name"));
    }
    if let Err(message) = warm_rules(profile, vm_prefix) {
        return Err(invalid(message));
    }
    if profile.job_name.is_empty()
        || profile.job_name.len() > 128
        || profile.job_name.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(invalid("job name contains an unsafe value"));
    }
    if !profile.allowed_events.iter().all(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err(invalid("event allowlist contains an unsafe name"));
    }
    if !profile
        .allowed_refs
        .iter()
        .chain(&profile.allowed_ref_prefixes)
        .all(|value| is_safe_git_ref(value))
    {
        return Err(invalid("ref allowlist contains an unsafe value"));
    }
    if !(1..=64).contains(&profile.cpu_count) || !(2_048..=131_072).contains(&profile.memory_mb) {
        return Err(invalid("CPU or memory is outside the allowed range"));
    }
    for (label, timeout, max) in [
        ("boot", profile.boot_timeout_seconds, 1_800),
        ("idle", profile.idle_timeout_seconds, 3_600),
        ("job", profile.job_timeout_seconds, 43_200),
        ("cleanup", profile.cleanup_timeout_seconds, 600),
    ] {
        if !(5..=max).contains(&timeout) {
            return Err(invalid(&format!(
                "{label} timeout is outside the allowed range"
            )));
        }
    }
    Ok(())
}

/// Warm production is declared in one place: an image name and the single
/// workflow allowed to produce it.
fn warm_rules(profile: &Profile, vm_prefix: &str) -> Result<(), &'static str> {
    if profile.warm_template.is_some() != profile.regeneration_workflow.is_some() {
        return Err("warm_template and regeneration_workflow must be declared together");
    }
    if let Some(warm) = &profile.warm_template {
        if warm.as_str().starts_with(vm_prefix) {
            return Err("warm_template cannot use the service-owned VM prefix");
        }
        if warm.as_str().len() > VmName::MAX_LEN - RESERVED_SUFFIX_LEN {
            return Err("warm_template is too long for its reserved suffixes");
        }
        if warm == &profile.template {
            return Err("warm_template cannot be the profile template");
        }
    }
    if let Some(workflow) = &profile.regeneration_workflow
        && (!is_safe_workflow(workflow) || !profile.allowed_workflows.contains(workflow))
    {
        return Err("regeneration_workflow must be an allowed workflow");
    }
    Ok(())
}

fn is_safe_workflow(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
