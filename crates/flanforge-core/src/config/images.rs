use std::collections::BTreeSet;

use crate::{PREVIOUS_SUFFIX, STAGING_SUFFIX, VmName};

use super::{Config, ConfigError};

/// Every declared warm name and its reserved suffixes must be unique across
/// profiles, outside the service prefix, and disjoint from every template.
pub(super) fn ensure_images_valid(config: &Config) -> Result<(), ConfigError> {
    let templates = config
        .profiles
        .values()
        .map(|profile| profile.template.as_str())
        .collect::<BTreeSet<_>>();
    let prefix = config.runtime.vm_prefix.as_str();
    let mut reserved = BTreeSet::new();
    for (name, profile) in &config.profiles {
        let invalid = |message: &str| ConfigError::InvalidProfile {
            profile: name.to_string(),
            message: message.to_owned(),
        };
        let Some(warm) = &profile.warm_template else {
            continue;
        };
        for derived in derived_names(warm).map_err(|()| invalid("warm image name is too long"))? {
            if derived.as_str().starts_with(prefix) {
                return Err(invalid("warm image name cannot use the service VM prefix"));
            }
            if templates.contains(derived.as_str()) {
                return Err(invalid(
                    "warm image name collides with a configured template",
                ));
            }
            if !reserved.insert(derived) {
                return Err(invalid("warm image name is already reserved by a profile"));
            }
        }
    }
    Ok(())
}

fn derived_names(warm: &VmName) -> Result<[VmName; 3], ()> {
    Ok([
        warm.clone(),
        warm.with_suffix(PREVIOUS_SUFFIX).map_err(|_| ())?,
        warm.with_suffix(STAGING_SUFFIX).map_err(|_| ())?,
    ])
}
