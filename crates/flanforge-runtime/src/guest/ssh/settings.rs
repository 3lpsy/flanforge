use std::path::PathBuf;

use flanforge_core::GuestConfig;
use flanforge_manager::WorkerError;

/// Everything the SSH channel needs, resolved once at construction so no later
/// call has to handle an absent setting.
#[derive(Clone, Debug)]
pub(super) struct SshSettings {
    pub(super) user: String,
    pub(super) identity_file: PathBuf,
    pub(super) connect_timeout_seconds: u64,
    pub(super) verify_host_key: bool,
    pub(super) known_hosts_file: Option<PathBuf>,
    pub(super) host_key_alias: Option<String>,
}

impl SshSettings {
    /// # Errors
    /// Returns an error when the guest configuration has no `[guest.ssh]`
    /// table, which only a channel that needs no SSH is allowed to omit.
    pub(super) fn from_config(config: &GuestConfig) -> Result<Self, WorkerError> {
        let ssh = config.ssh.as_ref().ok_or_else(|| {
            WorkerError::new("the SSH guest channel needs a [guest.ssh] configuration")
        })?;
        Ok(Self {
            user: config.runner_user.clone(),
            identity_file: ssh.identity_file.clone(),
            connect_timeout_seconds: ssh.connect_timeout_seconds,
            verify_host_key: ssh.verify_host_key,
            known_hosts_file: ssh.known_hosts_file.clone(),
            host_key_alias: ssh.host_key_alias.clone(),
        })
    }

    /// The privileged automation account over the same transport, with its
    /// own key when one is configured and the job key otherwise.
    pub(super) fn privileged_from_config(config: &GuestConfig) -> Result<Self, WorkerError> {
        let ssh = config.ssh.as_ref().ok_or_else(|| {
            WorkerError::new("the SSH guest channel needs a [guest.ssh] configuration")
        })?;
        Ok(Self {
            user: config.privileged_user.clone(),
            identity_file: ssh
                .privileged_identity_file
                .clone()
                .unwrap_or_else(|| ssh.identity_file.clone()),
            connect_timeout_seconds: ssh.connect_timeout_seconds,
            verify_host_key: ssh.verify_host_key,
            known_hosts_file: ssh.known_hosts_file.clone(),
            host_key_alias: ssh.host_key_alias.clone(),
        })
    }

    /// Re-pins the anchor to one allocation's generated host key, which the
    /// guest was seeded with rather than configured with.
    pub(super) fn pinned(&self, known_hosts_file: PathBuf, host_key_alias: String) -> Self {
        Self {
            verify_host_key: true,
            known_hosts_file: Some(known_hosts_file),
            host_key_alias: Some(host_key_alias),
            ..self.clone()
        }
    }
}
