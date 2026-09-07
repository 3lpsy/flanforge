use super::{Config, GuestSshConfig, Profile};

type Difference = (bool, &'static str);

/// Fields the native service definition bakes in at install time, so a restart
/// alone re-reads the old value. `shutdown_grace_seconds` becomes launchd's
/// `ExitTimeOut` and systemd's `TimeoutStopSec`; `db.db_path` is baked into the
/// systemd unit's `ReadWritePaths`.
const SERVICE_DEFINITION_FIELDS: &[&str] = &["server.shutdown_grace_seconds", "db.db_path"];

/// Returns whether applying a changed field needs `daemon install`, not just a
/// restart.
#[must_use]
pub fn is_service_definition_field(field: &str) -> bool {
    SERVICE_DEFINITION_FIELDS.contains(&field)
}

/// Names every restart-only field whose value differs, in declaration order.
///
/// A reload applies `[profiles]`, `[logging]`, the host budget, and the sweep
/// period; everything else is captured by a component built once at startup.
#[must_use]
pub fn restart_only_differences(old: &Config, new: &Config) -> Vec<&'static str> {
    server_differences(old, new)
        .into_iter()
        .chain(db_differences(old, new))
        .chain(webui_differences(old, new))
        .chain(endpoint_differences(old, new))
        .chain(runtime_differences(old, new))
        .chain(guest_differences(old, new))
        .filter_map(|(differs, field)| differs.then_some(field))
        .collect()
}

/// Whether a reload's change to this profile invalidates a hot machine already
/// running under it. A running machine cannot be resized under a claim, and a
/// machine sized by a superseded profile is not the machine the operator
/// configured. One predicate with its own tests, rather than a field list
/// open-coded at the drain site.
#[must_use]
pub fn is_hot_draining_change(old: &Profile, new: &Profile) -> bool {
    old.cpu_count != new.cpu_count
        || old.memory_mb != new.memory_mb
        || old.storage_mb != new.storage_mb
        || old.network != new.network
        || old.template != new.template
        || old.warm_template != new.warm_template
        || old.hot != new.hot
}

fn server_differences(old: &Config, new: &Config) -> Vec<Difference> {
    let (old, new) = (&old.server, &new.server);
    vec![
        (old.listen != new.listen, "server.listen"),
        (
            old.request_body_limit_bytes != new.request_body_limit_bytes,
            "server.request_body_limit_bytes",
        ),
        (
            old.allocation_wait_seconds != new.allocation_wait_seconds,
            "server.allocation_wait_seconds",
        ),
        (
            old.shutdown_grace_seconds != new.shutdown_grace_seconds,
            "server.shutdown_grace_seconds",
        ),
    ]
}

fn db_differences(old: &Config, new: &Config) -> Vec<Difference> {
    vec![(old.db.db_path != new.db.db_path, "db.db_path")]
}

// `webui.public_read_only` is deliberately absent: guards read it through the
// reloadable handle, so a reload applies it.
fn webui_differences(old: &Config, new: &Config) -> Vec<Difference> {
    let (old, new) = (&old.webui, &new.webui);
    vec![
        (old.enabled != new.enabled, "webui.enabled"),
        (
            old.session_ttl_seconds != new.session_ttl_seconds,
            "webui.session_ttl_seconds",
        ),
        (
            old.request_body_limit_bytes != new.request_body_limit_bytes,
            "webui.request_body_limit_bytes",
        ),
        (old.dev_dist_dir != new.dev_dist_dir, "webui.dev_dist_dir"),
        (
            old.authdb.enabled != new.authdb.enabled,
            "webui.authdb.enabled",
        ),
        // Any change to the relying-party table is reported once, by its name:
        // the client, secret, and endpoints are read when the surface starts.
        (old.oidc != new.oidc, "webui.oidc"),
    ]
}

fn endpoint_differences(old: &Config, new: &Config) -> Vec<Difference> {
    let (old_oidc, new_oidc) = (&old.oidc, &new.oidc);
    let (old_forgejo, new_forgejo) = (&old.forgejo, &new.forgejo);
    vec![
        (old_oidc.issuer != new_oidc.issuer, "oidc.issuer"),
        (old_oidc.audience != new_oidc.audience, "oidc.audience"),
        (old_oidc.jwks_url != new_oidc.jwks_url, "oidc.jwks_url"),
        (
            old_oidc.jwks_cache_seconds != new_oidc.jwks_cache_seconds,
            "oidc.jwks_cache_seconds",
        ),
        (
            old_oidc.clock_skew_seconds != new_oidc.clock_skew_seconds,
            "oidc.clock_skew_seconds",
        ),
        (
            old_forgejo.api_url != new_forgejo.api_url,
            "forgejo.api_url",
        ),
        (
            old_forgejo.api_token_file != new_forgejo.api_token_file,
            "forgejo.api_token_file",
        ),
        (
            old_forgejo.http_timeout_seconds != new_forgejo.http_timeout_seconds,
            "forgejo.http_timeout_seconds",
        ),
    ]
}

fn runtime_differences(old: &Config, new: &Config) -> Vec<Difference> {
    let (old, new) = (&old.runtime, &new.runtime);
    vec![
        (old.state_dir != new.state_dir, "runtime.state_dir"),
        (old.backend != new.backend, "runtime.backend"),
        (old.ssh_path != new.ssh_path, "runtime.ssh_path"),
        (old.scp_path != new.scp_path, "runtime.scp_path"),
        (old.vm_prefix != new.vm_prefix, "runtime.vm_prefix"),
        (
            old.max_running_vms != new.max_running_vms,
            "runtime.max_running_vms",
        ),
        (old.poll_seconds != new.poll_seconds, "runtime.poll_seconds"),
        // Only the zero transition is restart-only: the task is not spawned
        // when the sweep starts disabled, so nothing exists for a reload to
        // re-enable.
        (
            old.reap_interval_hours == 0 && new.reap_interval_hours != 0,
            "runtime.reap_interval_hours",
        ),
    ]
}

fn guest_differences(old: &Config, new: &Config) -> Vec<Difference> {
    let (old_guest, new_guest) = (&old.guest, &new.guest);
    let (old_tailscale, new_tailscale) = (&old.tailscale, &new.tailscale);
    let (old_ssh, new_ssh) = (old_guest.ssh.as_ref(), new_guest.ssh.as_ref());
    vec![
        (old_guest.channel != new_guest.channel, "guest.channel"),
        (
            old_guest.runner_user != new_guest.runner_user,
            "guest.runner_user",
        ),
        (
            old_guest.privileged_user != new_guest.privileged_user,
            "guest.privileged_user",
        ),
        (
            old_guest.forgejo_runner_path != new_guest.forgejo_runner_path,
            "guest.forgejo_runner_path",
        ),
        // A table that appears or disappears is reported once, by its own name:
        // the dotted keys below have nothing to compare against.
        (old_ssh.is_some() != new_ssh.is_some(), "guest.ssh"),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.identity_file),
            "guest.ssh.identity_file",
        ),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.privileged_identity_file),
            "guest.ssh.privileged_identity_file",
        ),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.known_hosts_file),
            "guest.ssh.known_hosts_file",
        ),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.host_key_alias),
            "guest.ssh.host_key_alias",
        ),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.connect_timeout_seconds),
            "guest.ssh.connect_timeout_seconds",
        ),
        (
            is_ssh_field_changed(old_ssh, new_ssh, |ssh| &ssh.verify_host_key),
            "guest.ssh.verify_host_key",
        ),
        (
            old_tailscale.enabled != new_tailscale.enabled,
            "tailscale.enabled",
        ),
        (
            old_tailscale.preauth_key_file != new_tailscale.preauth_key_file,
            "tailscale.preauth_key_file",
        ),
        (
            old_tailscale.login_server != new_tailscale.login_server,
            "tailscale.login_server",
        ),
        (
            old_tailscale.hostname != new_tailscale.hostname,
            "tailscale.hostname",
        ),
        (
            old_tailscale.extra_args != new_tailscale.extra_args,
            "tailscale.extra_args",
        ),
    ]
}

/// A field inside `[guest.ssh]` differs only when both documents have the
/// table; its appearance or removal is reported as `guest.ssh` instead.
fn is_ssh_field_changed<T: PartialEq + ?Sized>(
    old: Option<&GuestSshConfig>,
    new: Option<&GuestSshConfig>,
    field: impl for<'a> Fn(&'a GuestSshConfig) -> &'a T,
) -> bool {
    matches!((old, new), (Some(old), Some(new)) if field(old) != field(new))
}
