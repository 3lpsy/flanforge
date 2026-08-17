use super::{
    ConfigError, GuestConfig,
    validate::{ensure_path, ensure_range, ensure_safe_name},
};

pub(super) fn ensure_guest_valid(guest: &GuestConfig) -> Result<(), ConfigError> {
    ensure_safe_name("guest.ssh_user", &guest.ssh_user, 1, 64)?;
    ensure_path("guest.ssh_identity_file", &guest.ssh_identity_file)?;
    ensure_path("guest.forgejo_runner_path", &guest.forgejo_runner_path)?;
    ensure_range(
        "guest.ssh_connect_timeout_seconds",
        &guest.ssh_connect_timeout_seconds,
        1,
        60,
    )?;
    ensure_host_key_pinning_valid(guest)
}

/// The anchor and alias are read only while verification is enabled, so they
/// are required and validated only in that mode.
fn ensure_host_key_pinning_valid(guest: &GuestConfig) -> Result<(), ConfigError> {
    if !guest.verify_host_key {
        return Ok(());
    }
    let alias = guest
        .ssh_host_key_alias
        .as_deref()
        .ok_or(ConfigError::MissingGuestSetting {
            field: "ssh_host_key_alias",
        })?;
    ensure_safe_name("guest.ssh_host_key_alias", alias, 1, 128)?;
    let known_hosts =
        guest
            .ssh_known_hosts_file
            .as_deref()
            .ok_or(ConfigError::MissingGuestSetting {
                field: "ssh_known_hosts_file",
            })?;
    ensure_path("guest.ssh_known_hosts_file", known_hosts)
}
