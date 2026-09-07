#[cfg(test)]
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Backend,
    Bool,
    Integer,
    OptionalInteger,
    String,
    OptionalString,
    StringArray,
}

impl FieldKind {
    /// The name the web UI's schema view carries for this kind.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Backend => "backend",
            Self::Bool => "bool",
            Self::Integer => "integer",
            Self::OptionalInteger => "optional_integer",
            Self::String => "string",
            Self::OptionalString => "optional_string",
            Self::StringArray => "string_array",
        }
    }
}

const FIXED_FIELDS: &[(&str, FieldKind)] = &[
    ("db.db_path", FieldKind::OptionalString),
    ("forgejo.api_token_file", FieldKind::String),
    ("forgejo.api_url", FieldKind::String),
    ("forgejo.http_timeout_seconds", FieldKind::Integer),
    ("guest.channel", FieldKind::String),
    ("guest.forgejo_runner_path", FieldKind::String),
    ("guest.privileged_user", FieldKind::String),
    ("guest.runner_user", FieldKind::String),
    ("guest.ssh.connect_timeout_seconds", FieldKind::Integer),
    ("guest.ssh.host_key_alias", FieldKind::OptionalString),
    ("guest.ssh.identity_file", FieldKind::String),
    (
        "guest.ssh.privileged_identity_file",
        FieldKind::OptionalString,
    ),
    ("guest.ssh.known_hosts_file", FieldKind::OptionalString),
    ("guest.ssh.verify_host_key", FieldKind::Bool),
    ("logging.level", FieldKind::String),
    ("logging.path", FieldKind::OptionalString),
    ("oidc.audience", FieldKind::String),
    ("oidc.clock_skew_seconds", FieldKind::Integer),
    ("oidc.issuer", FieldKind::String),
    ("oidc.jwks_cache_seconds", FieldKind::Integer),
    ("oidc.jwks_url", FieldKind::String),
    ("runtime.backend.allow_insecure_transport", FieldKind::Bool),
    ("runtime.backend.home", FieldKind::OptionalString),
    (
        "runtime.backend.image_import_dir",
        FieldKind::OptionalString,
    ),
    ("runtime.backend.image_manifest_dir", FieldKind::String),
    ("runtime.backend.image_reimport", FieldKind::String),
    ("runtime.backend.kind", FieldKind::Backend),
    ("runtime.backend.min_storage_free_mb", FieldKind::Integer),
    ("runtime.backend.network", FieldKind::String),
    ("runtime.backend.path", FieldKind::String),
    ("runtime.backend.pool", FieldKind::String),
    ("runtime.backend.qemu_img_path", FieldKind::String),
    ("runtime.backend.runner_host_path", FieldKind::String),
    ("runtime.backend.uri", FieldKind::String),
    ("runtime.backend.virsh_path", FieldKind::String),
    (
        "runtime.backend.warm_capture_timeout_seconds",
        FieldKind::Integer,
    ),
    ("runtime.host_cpu_count", FieldKind::OptionalInteger),
    ("runtime.host_memory_mb", FieldKind::OptionalInteger),
    ("runtime.host_storage_mb", FieldKind::OptionalInteger),
    ("runtime.max_hot_vms", FieldKind::Integer),
    ("runtime.max_running_vms", FieldKind::Integer),
    ("runtime.poll_seconds", FieldKind::Integer),
    ("runtime.reap_interval_hours", FieldKind::Integer),
    ("runtime.scp_path", FieldKind::String),
    ("runtime.ssh_path", FieldKind::String),
    ("runtime.state_dir", FieldKind::String),
    ("runtime.vm_prefix", FieldKind::String),
    ("server.allocation_wait_seconds", FieldKind::Integer),
    ("server.listen", FieldKind::String),
    ("server.request_body_limit_bytes", FieldKind::Integer),
    ("server.shutdown_grace_seconds", FieldKind::Integer),
    ("tailscale.enabled", FieldKind::Bool),
    ("tailscale.extra_args", FieldKind::String),
    ("tailscale.hostname", FieldKind::OptionalString),
    ("tailscale.login_server", FieldKind::OptionalString),
    ("tailscale.preauth_key_file", FieldKind::OptionalString),
    ("webui.authdb.enabled", FieldKind::Bool),
    ("webui.dev_dist_dir", FieldKind::OptionalString),
    ("webui.enabled", FieldKind::Bool),
    ("webui.oidc.client_id", FieldKind::OptionalString),
    ("webui.oidc.client_secret_file", FieldKind::OptionalString),
    ("webui.oidc.enabled", FieldKind::Bool),
    ("webui.oidc.issuer", FieldKind::OptionalString),
    ("webui.oidc.redirect_url", FieldKind::OptionalString),
    ("webui.oidc.scopes", FieldKind::StringArray),
    ("webui.public_read_only", FieldKind::Bool),
    ("webui.request_body_limit_bytes", FieldKind::Integer),
    ("webui.session_ttl_seconds", FieldKind::Integer),
];

const PROFILE_FIELDS: &[(&str, FieldKind)] = &[
    ("allowed_events", FieldKind::StringArray),
    ("allowed_refs", FieldKind::StringArray),
    ("allowed_workflows", FieldKind::StringArray),
    ("boot_timeout_seconds", FieldKind::Integer),
    ("cleanup_timeout_seconds", FieldKind::Integer),
    ("cpu_count", FieldKind::Integer),
    ("idle_timeout_seconds", FieldKind::Integer),
    ("job_name", FieldKind::String),
    ("job_timeout_seconds", FieldKind::Integer),
    ("memory_mb", FieldKind::Integer),
    ("network", FieldKind::String),
    ("reap", FieldKind::Bool),
    ("regeneration_workflow", FieldKind::OptionalString),
    ("repository", FieldKind::String),
    ("require_protected_ref", FieldKind::Bool),
    ("runner_label", FieldKind::String),
    ("storage_mb", FieldKind::Integer),
    ("template", FieldKind::String),
    ("warm_template", FieldKind::OptionalString),
];

/// `[profiles.<name>.hot]`. The table is optional, so a profile with no hot
/// keys addresses none of these and still round-trips.
const PROFILE_HOT_FIELDS: &[(&str, FieldKind)] = &[
    ("enabled", FieldKind::Bool),
    ("idle_ttl_seconds", FieldKind::Integer),
    ("lanes", FieldKind::String),
    ("max_idle", FieldKind::Integer),
    ("max_jobs", FieldKind::Integer),
    ("max_lifetime_seconds", FieldKind::Integer),
    ("reset_timeout_seconds", FieldKind::Integer),
    ("simulator_reset", FieldKind::String),
];

pub(crate) fn field_kind(segments: &[&str]) -> Option<FieldKind> {
    if segments.len() == 4
        && segments[0] == "profiles"
        && !segments[1].is_empty()
        && segments[2] == "hot"
    {
        return PROFILE_HOT_FIELDS
            .iter()
            .find_map(|(field, kind)| (*field == segments[3]).then_some(*kind));
    }
    if segments.len() == 3 && segments[0] == "profiles" && !segments[1].is_empty() {
        return PROFILE_FIELDS
            .iter()
            .find_map(|(field, kind)| (*field == segments[2]).then_some(*kind));
    }
    let key = segments.join(".");
    FIXED_FIELDS
        .iter()
        .find_map(|(field, kind)| (*field == key).then_some(*kind))
}

/// Backend keys under `runtime.backend`, per kind; the view schema shows only
/// the active backend's keys.
const TART_BACKEND_FIELDS: &[&str] = &["kind", "path", "runner_host_path", "home"];
const LIBVIRT_BACKEND_FIELDS: &[&str] = &[
    "kind",
    "uri",
    "allow_insecure_transport",
    "pool",
    "network",
    "image_manifest_dir",
    "image_import_dir",
    "image_reimport",
    "min_storage_free_mb",
    "qemu_img_path",
    "virsh_path",
    "warm_capture_timeout_seconds",
];

/// Every supported key for the schema-driven web UI view: the fixed keys
/// narrowed to the active backend, then `profiles.*` patterns standing for
/// each profile and its optional hot table.
#[must_use]
pub fn view_fields(backend_kind: &str) -> Vec<(String, FieldKind)> {
    let backend_keys = if backend_kind == "libvirt" {
        LIBVIRT_BACKEND_FIELDS
    } else {
        TART_BACKEND_FIELDS
    };
    FIXED_FIELDS
        .iter()
        .filter(|(field, _)| {
            field
                .strip_prefix("runtime.backend.")
                .is_none_or(|key| backend_keys.contains(&key))
        })
        .map(|(field, kind)| ((*field).to_owned(), *kind))
        .chain(
            PROFILE_FIELDS
                .iter()
                .map(|(field, kind)| (format!("profiles.*.{field}"), *kind)),
        )
        .chain(
            PROFILE_HOT_FIELDS
                .iter()
                .map(|(field, kind)| (format!("profiles.*.hot.{field}"), *kind)),
        )
        .collect()
}

#[cfg(test)]
pub(crate) fn supported_field_patterns() -> BTreeSet<String> {
    FIXED_FIELDS
        .iter()
        .map(|(field, _)| (*field).to_owned())
        .chain(
            PROFILE_FIELDS
                .iter()
                .map(|(field, _)| format!("profiles.*.{field}")),
        )
        .chain(
            PROFILE_HOT_FIELDS
                .iter()
                .map(|(field, _)| format!("profiles.*.hot.{field}")),
        )
        .collect()
}
