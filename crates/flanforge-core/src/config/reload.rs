use super::Config;

type Difference = (bool, &'static str);

/// Names every restart-only field whose value differs, in declaration order.
///
/// A reload applies `[profiles]`, `[logging]`, the host budget, and the sweep
/// period; everything else is captured by a component built once at startup.
#[must_use]
pub fn restart_only_differences(old: &Config, new: &Config) -> Vec<&'static str> {
    server_differences(old, new)
        .into_iter()
        .chain(endpoint_differences(old, new))
        .chain(runtime_differences(old, new))
        .chain(guest_differences(old, new))
        .filter_map(|(differs, field)| differs.then_some(field))
        .collect()
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
        (old.tart_path != new.tart_path, "runtime.tart_path"),
        (old.ssh_path != new.ssh_path, "runtime.ssh_path"),
        (old.scp_path != new.scp_path, "runtime.scp_path"),
        (
            old.forgejo_runner_host_path != new.forgejo_runner_host_path,
            "runtime.forgejo_runner_host_path",
        ),
        (old.vm_prefix != new.vm_prefix, "runtime.vm_prefix"),
        (old.tart_home != new.tart_home, "runtime.tart_home"),
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
    vec![
        (old_guest.ssh_user != new_guest.ssh_user, "guest.ssh_user"),
        (
            old_guest.ssh_identity_file != new_guest.ssh_identity_file,
            "guest.ssh_identity_file",
        ),
        (
            old_guest.ssh_known_hosts_file != new_guest.ssh_known_hosts_file,
            "guest.ssh_known_hosts_file",
        ),
        (
            old_guest.ssh_host_key_alias != new_guest.ssh_host_key_alias,
            "guest.ssh_host_key_alias",
        ),
        (
            old_guest.forgejo_runner_path != new_guest.forgejo_runner_path,
            "guest.forgejo_runner_path",
        ),
        (
            old_guest.ssh_connect_timeout_seconds != new_guest.ssh_connect_timeout_seconds,
            "guest.ssh_connect_timeout_seconds",
        ),
        (
            old_guest.verify_host_key != new_guest.verify_host_key,
            "guest.verify_host_key",
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
