use std::{path::PathBuf, sync::Arc, time::Duration};

use flanforge_core::{GuestConfig, RuntimeBackendKind, TailscaleConfig};
use flanforge_manager::WorkerError;

use super::{GuestSession, ssh::SshChannel, tailscale};
use crate::channel::{GuestChannel, GuestCommand, GuestFileTransfer};

/// Everything the daemon does inside a guest, expressed once and carried by
/// whichever channel the backend bound.
#[derive(Clone, Debug)]
pub struct GuestControl {
    pub(super) channel: Arc<dyn GuestChannel>,
    pub(super) transfer: Option<Arc<dyn GuestFileTransfer>>,
    pub(super) runner_path: PathBuf,
    pub(super) runner_user: String,
    pub(super) tailscale: TailscaleConfig,
    pub(super) tailscale_path: &'static str,
}

impl GuestControl {
    /// # Errors
    /// Returns an error when the guest configuration cannot drive SSH.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        config: GuestConfig,
        tailscale: TailscaleConfig,
        ssh_path: PathBuf,
        scp_path: PathBuf,
    ) -> Result<Self, WorkerError> {
        Self::new_for_backend(
            config,
            tailscale,
            ssh_path,
            scp_path,
            RuntimeBackendKind::Tart,
        )
    }

    /// # Errors
    /// Returns an error when the guest configuration cannot drive SSH.
    pub fn new_for_backend(
        config: GuestConfig,
        tailscale: TailscaleConfig,
        ssh_path: PathBuf,
        scp_path: PathBuf,
        backend: RuntimeBackendKind,
    ) -> Result<Self, WorkerError> {
        let ssh = Arc::new(SshChannel::new(&config, ssh_path, scp_path)?);
        Ok(Self {
            channel: ssh.clone(),
            transfer: Some(ssh),
            runner_path: config.forgejo_runner_path,
            runner_user: config.runner_user,
            tailscale,
            tailscale_path: tailscale::path(backend),
        })
    }

    /// Builds control over a caller-supplied channel, for a backend whose
    /// channel is bound per allocation rather than once per daemon.
    #[must_use]
    pub fn with_channel(
        config: &GuestConfig,
        tailscale: TailscaleConfig,
        backend: RuntimeBackendKind,
        channel: Arc<dyn GuestChannel>,
        transfer: Option<Arc<dyn GuestFileTransfer>>,
    ) -> Self {
        Self {
            channel,
            transfer,
            runner_path: config.forgejo_runner_path.clone(),
            runner_user: config.runner_user.clone(),
            tailscale,
            tailscale_path: tailscale::path(backend),
        }
    }

    /// The bound channel, for a backend step that runs one command over
    /// whichever channel this control was built with.
    #[must_use]
    pub fn channel(&self) -> &dyn GuestChannel {
        self.channel.as_ref()
    }

    pub(crate) const fn is_transfer_bound(&self) -> bool {
        self.transfer.is_some()
    }

    /// Whether this control joins guests to the configured tailnet.
    #[must_use]
    pub const fn is_tailscale_managed(&self) -> bool {
        self.tailscale.enabled
    }

    /// Waits for one session using its pinned host-key policy.
    ///
    /// # Errors
    /// Returns an error when the session cannot become ready in time.
    pub async fn wait_session_ready(
        &self,
        session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        self.channel.ensure_ready(session, timeout).await
    }

    /// # Errors
    /// Returns an error for a malformed address, or when the guest does not
    /// become reachable before the timeout.
    pub async fn wait_ready(&self, ip: &str, timeout: Duration) -> Result<(), WorkerError> {
        self.wait_session_ready(&GuestSession::configured(ip)?, timeout)
            .await
    }

    /// Executes a caller-owned bounded script using one pinned session.
    ///
    /// # Errors
    /// Returns an error when validation, the channel, or the script fails.
    pub async fn run_session_script(
        &self,
        session: &GuestSession,
        script: &str,
    ) -> Result<(), WorkerError> {
        let command = GuestCommand::quiet(script)?;
        let exit = self
            .channel
            .run(session, &command)
            .await
            .map_err(|_| WorkerError::new("cannot execute guest daemon script"))?
            .exit();
        if exit.is_success() {
            Ok(())
        } else {
            // Scripts signal their gate through the exit code, so it is the
            // whole diagnosis when the output is discarded.
            Err(WorkerError::new(format!(
                "guest daemon script did not complete (exit {})",
                exit_label(exit)
            )))
        }
    }

    /// Executes a daemon-owned bounded script as the privileged automation
    /// account, for the few steps that need root inside the guest.
    ///
    /// # Errors
    /// Returns an error when the channel has no privileged path, the base
    /// bakes no such account, or the script fails.
    pub async fn run_privileged_session_script(
        &self,
        session: &GuestSession,
        script: &str,
    ) -> Result<(), WorkerError> {
        let command = GuestCommand::quiet(script)?;
        let exit = self.channel.run_privileged(session, &command).await?.exit();
        if exit.is_success() {
            Ok(())
        } else {
            Err(WorkerError::new(format!(
                "privileged guest script did not complete (exit {})",
                exit_label(exit)
            )))
        }
    }

    /// # Errors
    /// Returns an error for a malformed address, or when validation, the
    /// channel, or the script fails.
    pub(crate) async fn run_daemon_script(
        &self,
        address: &str,
        script: &str,
    ) -> Result<(), WorkerError> {
        self.run_session_script(&GuestSession::configured(address)?, script)
            .await
    }
}

/// One label for both wrapper errors: the code when there is one, the way the
/// process ended when there is not.
fn exit_label(exit: crate::channel::GuestExit) -> String {
    exit.code()
        .map_or_else(|| format!("{exit:?}"), |code| code.to_string())
}
