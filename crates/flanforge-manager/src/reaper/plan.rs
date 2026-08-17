use std::collections::BTreeSet;

use flanforge_core::{
    Allocation, AllocationId, AllocationMode, Config, PREVIOUS_SUFFIX, ProfileName, STAGING_SUFFIX,
    VmName, WarmImageRecord,
};
use serde::{Deserialize, Serialize};

use super::super::{HostMachine, MachineState};

/// Nothing younger than this is swept, so a clone being created right now is
/// never a candidate.
const MIN_AGE_SECONDS: u64 = 3_600;
/// An image needs a longer floor, so an in-progress configuration edit cannot
/// destroy the image a profile is being repointed at.
const IMAGE_AGE_SECONDS: u64 = 86_400;

/// What permits a deletion, logged with the action so any sweep greps back to
/// the record that authorized it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReapAuthorization {
    Record(AllocationId),
    Staging(ProfileName),
    Image(ProfileName),
    /// Reported at WARN, never deleted.
    Prefix,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReapCandidate {
    pub name: String,
    pub state: MachineState,
    pub authorization: ReapAuthorization,
    pub age_seconds: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ReapInputs<'a> {
    pub config: &'a Config,
    pub machines: &'a [HostMachine],
    pub allocations: &'a [Allocation],
    pub images: &'a [WarmImageRecord],
}

/// Turns one snapshot into typed candidates. Deletes nothing.
#[must_use]
pub fn plan_sweep(inputs: ReapInputs<'_>) -> Vec<ReapCandidate> {
    let protected = protected_names(&inputs);
    let prefix = inputs.config.runtime.vm_prefix.as_str();
    inputs
        .machines
        .iter()
        .filter(|machine| !protected.contains(&machine.name))
        .filter_map(|machine| candidate(&inputs, machine, prefix))
        .collect()
}

/// Names no sweep may consider: anything an active allocation is bound to,
/// every configured template, and every image a live profile still references.
fn protected_names(inputs: &ReapInputs<'_>) -> BTreeSet<String> {
    let mut protected = BTreeSet::new();
    for allocation in inputs.allocations {
        if allocation.state.is_terminal() {
            continue;
        }
        protected.insert(allocation.vm_name.to_string());
        // A regeneration in flight owns all three of its profile's names.
        if allocation.mode == AllocationMode::Regenerate
            && let Some(profile) = inputs.config.profiles.get(&allocation.request.profile)
            && let Some(warm) = &profile.warm_template
        {
            protected.extend(derived_names(warm));
        }
    }
    protected.extend(reserved_image_names(inputs.config));
    protected
}

/// Every image name live configuration claims. A candidate may never be one of
/// these, whatever record names it; `.staging` is deliberately absent, because
/// an abandoned candidate is exactly what the sweep collects.
#[must_use]
pub fn reserved_image_names(config: &Config) -> BTreeSet<String> {
    let mut reserved = BTreeSet::new();
    for profile in config.profiles.values() {
        reserved.insert(profile.template.to_string());
        if let Some(warm) = &profile.warm_template {
            reserved.insert(warm.to_string());
            reserved.extend(suffixed(warm, PREVIOUS_SUFFIX));
        }
    }
    reserved
}

fn candidate(
    inputs: &ReapInputs<'_>,
    machine: &HostMachine,
    prefix: &str,
) -> Option<ReapCandidate> {
    // An undeterminable age is skipped: the sweep never guesses at ownership.
    let age_seconds = machine.age_seconds?;
    if age_seconds < MIN_AGE_SECONDS {
        return None;
    }
    let authorization = if machine.name.starts_with(prefix) {
        clone_authorization(inputs, &machine.name)
    } else {
        image_authorization(inputs, &machine.name, age_seconds)?
    };
    Some(ReapCandidate {
        name: machine.name.clone(),
        state: machine.state,
        authorization,
        age_seconds,
    })
}

/// A prefix-shaped name is only deletable when a terminal record proves the
/// daemon created it and its cleanup never finished.
fn clone_authorization(inputs: &ReapInputs<'_>, name: &str) -> ReapAuthorization {
    inputs
        .allocations
        .iter()
        .find(|allocation| {
            allocation.state.is_terminal()
                && allocation.vm_created
                && allocation.vm_name.as_str() == name
        })
        .map_or(ReapAuthorization::Prefix, |allocation| {
            ReapAuthorization::Record(allocation.id)
        })
}

/// Outside the prefix the daemon owns only the names its own records claim.
fn image_authorization(
    inputs: &ReapInputs<'_>,
    name: &str,
    age_seconds: u64,
) -> Option<ReapAuthorization> {
    for (profile_name, profile) in &inputs.config.profiles {
        if !profile.reap {
            continue;
        }
        if profile
            .warm_template
            .as_ref()
            .is_some_and(|warm| suffixed(warm, STAGING_SUFFIX).is_some_and(|value| value == name))
        {
            return Some(ReapAuthorization::Staging(profile_name.clone()));
        }
    }
    if age_seconds < IMAGE_AGE_SECONDS {
        return None;
    }
    inputs
        .images
        .iter()
        .filter(|record| is_reapable_profile(inputs.config, &record.profile))
        .find(|record| derived_names(&record.warm_template).contains(name))
        .map(|record| ReapAuthorization::Image(record.profile.clone()))
}

/// A profile that opted out keeps its images; a profile that is gone cannot.
fn is_reapable_profile(config: &Config, profile: &ProfileName) -> bool {
    config
        .profiles
        .get(profile)
        .is_none_or(|profile| profile.reap)
}

fn derived_names(warm: &VmName) -> BTreeSet<String> {
    let mut names = BTreeSet::from([warm.to_string()]);
    names.extend(suffixed(warm, PREVIOUS_SUFFIX));
    names.extend(suffixed(warm, STAGING_SUFFIX));
    names
}

fn suffixed(warm: &VmName, suffix: &str) -> Option<String> {
    warm.with_suffix(suffix)
        .ok()
        .map(|name| name.as_str().to_owned())
}
