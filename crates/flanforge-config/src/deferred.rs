use std::path::Path;

use crate::{
    ConfigLoadError, STARTER_CONFIG,
    decode::decode_config_with_overrides,
    expand::{expand_string, expand_value},
};

/// The baseline must resolve without an environment, so its tilde paths need a
/// home that never comes from one.
const BASELINE_HOME: &str = "/";

/// Validates the profiles of a document whose deployment this environment
/// cannot resolve, by judging them in the shipped baseline deployment instead.
///
/// # Errors
/// Returns an error when a profile is malformed, breaks policy, or itself
/// depends on a variable this environment does not define.
pub(crate) fn ensure_profiles_valid(
    document: &toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigLoadError> {
    ensure_profiles_resolve(document, environment)?;
    let value = baseline_with_profiles(document, environment)?;
    let config =
        decode_config_with_overrides(value, environment, &[], &[], Some(Path::new(BASELINE_HOME)))?;
    config.ensure_valid().map_err(ConfigLoadError::Validation)
}

/// The operator's profiles, in a deployment that is known to be valid.
fn baseline_with_profiles(
    document: &toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<toml::Value, ConfigLoadError> {
    let mut baseline =
        toml::from_str::<toml::Table>(STARTER_CONFIG).map_err(ConfigLoadError::Toml)?;
    baseline.remove("profiles");
    if let Some(profiles) = document.get("profiles") {
        baseline.insert("profiles".to_owned(), profiles.clone());
    }
    if let Some(prefix) = resolvable_vm_prefix(document, environment)
        && let Some(runtime) = baseline
            .get_mut("runtime")
            .and_then(toml::Value::as_table_mut)
    {
        runtime.insert(
            "vm_prefix".to_owned(),
            toml::Value::String(prefix.to_owned()),
        );
    }
    Ok(toml::Value::Table(baseline))
}

/// Profile policy reads the service VM prefix, so profiles are judged against
/// the operator's own prefix whenever this environment can resolve it.
fn resolvable_vm_prefix<'a>(
    document: &'a toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Option<&'a str> {
    let prefix = document.get("runtime")?.get("vm_prefix")?.as_str()?;
    expand_string(prefix, environment).is_ok().then_some(prefix)
}

/// A placeholder the mutation is responsible for still fails the save, and
/// names the profile holding it rather than the document as a whole.
fn ensure_profiles_resolve(
    document: &toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigLoadError> {
    let Some(profiles) = document.get("profiles").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for (name, profile) in profiles {
        let mut value = profile.clone();
        expand_value(&mut value, environment).map_err(|error| name_profile(name, error))?;
    }
    Ok(())
}

fn name_profile(profile: &str, error: ConfigLoadError) -> ConfigLoadError {
    match error {
        ConfigLoadError::MissingEnvironment(variable) => {
            ConfigLoadError::UnresolvedProfilePlaceholder {
                profile: profile.to_owned(),
                variable,
            }
        }
        other => other,
    }
}
