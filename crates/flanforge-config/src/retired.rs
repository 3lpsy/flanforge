use crate::ConfigLoadError;

/// Profile keys removed from the schema, each with the migration it needs.
///
/// Every section denies unknown fields, so a document still naming one of these
/// would otherwise stop the daemon with nothing but "unknown field".
const RETIRED_PROFILE_FIELDS: &[(&str, &str)] = &[(
    "allowed_ref_prefixes",
    "move each entry into `allowed_refs` as a glob by appending `*` \
     (a prefix `refs/tags/v` becomes `refs/tags/v*`)",
)];

/// Document-wide keys removed from the schema, addressed by their dotted path.
const RETIRED_FIELDS: &[(&str, &str)] = &[
    (
        "guest.ssh_user",
        "renamed to `guest.runner_user`: it names the guest account the job runs \
         as and Tailscale's `--operator`, in both channels",
    ),
    (
        "guest.ssh_identity_file",
        "moved to `[guest.ssh].identity_file`",
    ),
    (
        "guest.ssh_known_hosts_file",
        "moved to `[guest.ssh].known_hosts_file`",
    ),
    (
        "guest.ssh_host_key_alias",
        "moved to `[guest.ssh].host_key_alias`",
    ),
    (
        "guest.ssh_connect_timeout_seconds",
        "moved to `[guest.ssh].connect_timeout_seconds`",
    ),
    (
        "guest.verify_host_key",
        "moved to `[guest.ssh].verify_host_key`",
    ),
];

/// The migration text for a retired profile key, for callers that reject the
/// key before it ever reaches a document.
#[must_use]
pub fn retired_profile_field(field: &str) -> Option<&'static str> {
    RETIRED_PROFILE_FIELDS
        .iter()
        .find_map(|(name, migration)| (*name == field).then_some(*migration))
}

/// Names the profile and the replacement before deserialization reports the key
/// as merely unknown.
pub(crate) fn ensure_no_retired_profile_fields(value: &toml::Value) -> Result<(), ConfigLoadError> {
    let Some(profiles) = value.get("profiles").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for (profile, table) in profiles {
        let Some(table) = table.as_table() else {
            continue;
        };
        for (field, migration) in RETIRED_PROFILE_FIELDS {
            if table.contains_key(*field) {
                return Err(ConfigLoadError::RetiredProfileField {
                    profile: profile.clone(),
                    field,
                    migration,
                });
            }
        }
    }
    Ok(())
}

/// Same service for the fixed tables, whose keys are addressed by dotted path.
pub(crate) fn ensure_no_retired_fields(value: &toml::Value) -> Result<(), ConfigLoadError> {
    for (field, migration) in RETIRED_FIELDS {
        if is_present(value, field) {
            return Err(ConfigLoadError::RetiredField { field, migration });
        }
    }
    Ok(())
}

fn is_present(value: &toml::Value, field: &str) -> bool {
    field
        .split('.')
        .try_fold(value, |current, segment| current.get(segment))
        .is_some()
}
