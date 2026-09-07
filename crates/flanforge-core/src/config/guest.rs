use super::{
    Config, ConfigError, GuestChannelKind, GuestSshConfig, RuntimeBackendConfig,
    RuntimeBackendKind,
    validate::{ensure_path, ensure_range, ensure_safe_name},
};

pub(super) fn ensure_guest_valid(config: &Config) -> Result<(), ConfigError> {
    let guest = &config.guest;
    ensure_safe_name("guest.runner_user", &guest.runner_user, 1, 64)?;
    ensure_safe_name("guest.privileged_user", &guest.privileged_user, 1, 64)?;
    // Root needs no grant, and sharing the job account's name would hand
    // every job the privileged account's sudo rule.
    if guest.privileged_user == "root" || guest.privileged_user == guest.runner_user {
        return Err(ConfigError::UnsafeValue {
            field: "guest.privileged_user",
        });
    }
    ensure_path("guest.forgejo_runner_path", &guest.forgejo_runner_path)?;
    ensure_channel_supported(config)?;
    let backend = config.runtime.backend_kind();
    match (&guest.ssh, guest.channel) {
        (Some(ssh), _) => ensure_ssh_valid(backend, ssh),
        (None, GuestChannelKind::Agent) => Ok(()),
        (None, GuestChannelKind::Ssh) => Err(ConfigError::MissingGuestTable {
            table: "guest.ssh",
            backend,
            channel: guest.channel,
        }),
    }
}

/// Every rule here runs whenever `[guest.ssh]` is present, whatever the
/// channel: selecting the agent channel must never be a way to smuggle a weak
/// SSH configuration past validation, and flipping back to `ssh` must never
/// turn a valid document into an unsafe one.
fn ensure_ssh_valid(backend: RuntimeBackendKind, ssh: &GuestSshConfig) -> Result<(), ConfigError> {
    ensure_path("guest.ssh.identity_file", &ssh.identity_file)?;
    ensure_range(
        "guest.ssh.connect_timeout_seconds",
        &ssh.connect_timeout_seconds,
        1,
        60,
    )?;
    match backend {
        RuntimeBackendKind::Tart => ensure_host_key_pinning_valid(ssh),
        RuntimeBackendKind::Libvirt => ensure_allocation_pinning_valid(ssh),
    }
}

/// A resolved `agent` over `qemu+tcp` is impossible — the default is `ssh`
/// there — so this refuses only a channel the operator wrote by hand.
fn ensure_channel_supported(config: &Config) -> Result<(), ConfigError> {
    if config.guest.channel == GuestChannelKind::Ssh {
        return Ok(());
    }
    match &config.runtime.backend {
        RuntimeBackendConfig::Tart(_) => Err(ConfigError::UnsupportedBackendCapability {
            backend: "tart",
            capability: "the QEMU guest agent channel",
            profile: None,
        }),
        RuntimeBackendConfig::Libvirt(libvirt) if libvirt.is_clear_text_transport() => {
            Err(ConfigError::InsecureGuestChannelTransport)
        }
        RuntimeBackendConfig::Libvirt(_) => Ok(()),
    }
}

/// The account name is the operator's: the seed only drops an authorized key
/// into its home, and the base image is what actually provides the account.
/// The host-key anchor is not negotiable, because libvirt pins a freshly
/// generated key per allocation rather than a long-lived one.
fn ensure_allocation_pinning_valid(ssh: &GuestSshConfig) -> Result<(), ConfigError> {
    if !ssh.verify_host_key {
        return Err(ConfigError::UnsafeValue {
            field: "guest.ssh.verify_host_key",
        });
    }
    if ssh.known_hosts_file.is_some() || ssh.host_key_alias.is_some() {
        return Err(ConfigError::UnsafeValue {
            field: "guest allocation host-key anchor",
        });
    }
    Ok(())
}

/// The anchor and alias are read only while verification is enabled, so they
/// are required and validated only in that mode.
fn ensure_host_key_pinning_valid(ssh: &GuestSshConfig) -> Result<(), ConfigError> {
    if !ssh.verify_host_key {
        return Ok(());
    }
    let alias = ssh
        .host_key_alias
        .as_deref()
        .ok_or(ConfigError::MissingGuestSetting {
            field: "host_key_alias",
        })?;
    ensure_safe_name("guest.ssh.host_key_alias", alias, 1, 128)?;
    let known_hosts = ssh
        .known_hosts_file
        .as_deref()
        .ok_or(ConfigError::MissingGuestSetting {
            field: "known_hosts_file",
        })?;
    ensure_path("guest.ssh.known_hosts_file", known_hosts)
}
