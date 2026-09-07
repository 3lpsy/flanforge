use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use flanforge_core::{Allocation, Config, RuntimeBackendKind};
use flanforge_forgejo::ForgejoClient;
use flanforge_manager::WorkerError;
use tokio_util::sync::CancellationToken;

use super::RunnerRegistration;
use crate::{
    channel::{GuestChannel, GuestFileTransfer, GuestProcess},
    guest::{GuestControl, GuestSession},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerDelivery {
    HostCopy(PathBuf),
    Image,
}

/// A started guest runner and what waiting for its job left of the idle TTL, so
/// supervision continues one idle budget instead of opening a second.
#[derive(Debug)]
pub struct StartedRunner {
    pub process: Box<dyn GuestProcess>,
    pub idle_deadline: tokio::time::Instant,
}

#[derive(Clone, Debug)]
pub struct GuestJob {
    guest: GuestControl,
    registration: RunnerRegistration,
    poll: Duration,
    backend: RuntimeBackendKind,
    delivery: RunnerDelivery,
}

impl GuestJob {
    /// # Errors
    /// Returns an error when the guest configuration cannot drive SSH.
    pub fn new(
        config: &Config,
        forgejo: ForgejoClient,
        delivery: RunnerDelivery,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            guest: GuestControl::new_for_backend(
                config.guest.clone(),
                config.tailscale.clone(),
                config.runtime.ssh_path.clone(),
                config.runtime.scp_path.clone(),
                config.runtime.backend_kind(),
            )?,
            registration: RunnerRegistration::new(forgejo),
            poll: Duration::from_secs(config.runtime.poll_seconds),
            backend: config.runtime.backend_kind(),
            delivery,
        })
    }

    /// Binds one caller-built channel. A backend whose channel is per-domain
    /// cannot build the job once at open time, so it builds one per allocation
    /// and reuses the registration it opened with.
    #[must_use]
    pub fn with_channel(
        config: &Config,
        registration: RunnerRegistration,
        delivery: RunnerDelivery,
        channel: Arc<dyn GuestChannel>,
        transfer: Option<Arc<dyn GuestFileTransfer>>,
    ) -> Self {
        Self {
            guest: GuestControl::with_channel(
                &config.guest,
                config.tailscale.clone(),
                config.runtime.backend_kind(),
                channel,
                transfer,
            ),
            registration,
            poll: Duration::from_secs(config.runtime.poll_seconds),
            backend: config.runtime.backend_kind(),
            delivery,
        }
    }

    /// Waits for the guest control channel using the backend's required trust.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched trust strategy or an unavailable guest.
    pub async fn ensure_ready(
        &self,
        session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        self.ensure_channel_ready(session, timeout).await?;
        self.ensure_tailscale_ready(session).await
    }

    /// Waits until the channel can carry commands. Reachability only: whether
    /// first-boot provisioning finished is a property of the image, so its gate
    /// belongs to the backend that baked it.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched trust strategy or an unavailable guest.
    pub async fn ensure_channel_ready(
        &self,
        session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        self.ensure_session(session)?;
        self.guest.wait_session_ready(session, timeout).await
    }

    /// Joins the tailnet, when the operator configured one.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched trust strategy, or when the join fails.
    pub async fn ensure_tailscale_ready(&self, session: &GuestSession) -> Result<(), WorkerError> {
        self.ensure_session(session)?;
        self.guest.ensure_tailscale_connected(session).await
    }

    /// The channel this job drives its guest over, for a backend step that has
    /// to run one command over whichever channel is bound.
    #[must_use]
    pub fn channel(&self) -> &dyn GuestChannel {
        self.guest.channel()
    }

    /// # Errors
    /// Returns an error when the phase is cancelled, or when it does not
    /// finish before the deadline.
    pub async fn phase<T, F>(
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
        timeout_message: &'static str,
        future: F,
    ) -> Result<T, WorkerError>
    where
        F: Future<Output = Result<T, WorkerError>>,
    {
        tokio::select! {
            () = cancellation.cancelled() => Err(WorkerError::new("allocation was cancelled")),
            result = tokio::time::timeout_at(deadline, future) => {
                result.map_err(|_| WorkerError::new(timeout_message))?
            }
        }
    }

    /// Executes a fixed, backend-owned maintenance script in the guest.
    ///
    /// # Errors
    ///
    /// Returns an error when the script is invalid or does not complete.
    pub async fn run_daemon_script(&self, address: &str, script: &str) -> Result<(), WorkerError> {
        self.guest.run_daemon_script(address, script).await
    }

    /// Executes a fixed, backend-owned script over one pinned session, so an
    /// allocation-pinned guest keeps its host-key policy.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched trust strategy, or when the script is
    /// invalid or does not complete.
    pub async fn run_session_script(
        &self,
        session: &GuestSession,
        script: &str,
    ) -> Result<(), WorkerError> {
        self.ensure_session(session)?;
        self.guest.run_session_script(session, script).await
    }

    /// Runs one daemon-owned script as the privileged automation account.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched trust strategy, when the channel has
    /// no privileged path, or when the script does not complete.
    pub async fn run_privileged_session_script(
        &self,
        session: &GuestSession,
        script: &str,
    ) -> Result<(), WorkerError> {
        self.ensure_session(session)?;
        self.guest
            .run_privileged_session_script(session, script)
            .await
    }

    /// Deletes the exact runner registration owned by one allocation.
    ///
    /// # Errors
    ///
    /// Returns an error when Forgejo refuses or cannot complete the cleanup.
    pub async fn delete_runner(&self, allocation: &Allocation) -> Result<(), WorkerError> {
        self.registration.delete_runner(allocation).await
    }

    pub(crate) const fn forgejo_client(&self) -> &ForgejoClient {
        self.registration.client()
    }

    pub(crate) const fn poll(&self) -> Duration {
        self.poll
    }

    pub(super) const fn guest(&self) -> &GuestControl {
        &self.guest
    }

    /// Whether this job joins guests to the configured tailnet.
    #[must_use]
    pub const fn is_tailscale_managed(&self) -> bool {
        self.guest.is_tailscale_managed()
    }

    pub(super) const fn delivery(&self) -> &RunnerDelivery {
        &self.delivery
    }

    pub(super) fn ensure_session(&self, session: &GuestSession) -> Result<(), WorkerError> {
        // A host-copied runner needs a file transport, and a channel without
        // one would otherwise leave the guest with no runner at all.
        if matches!(self.delivery, RunnerDelivery::HostCopy(_)) && !self.guest.is_transfer_bound() {
            return Err(WorkerError::new(
                "the guest control channel cannot deliver a host-copied runner",
            ));
        }
        let compatible = matches!(
            (
                &self.backend,
                &self.delivery,
                session.is_allocation_pinned()
            ),
            (RuntimeBackendKind::Tart, _, false)
                | (RuntimeBackendKind::Libvirt, RunnerDelivery::Image, true)
        );
        if compatible {
            Ok(())
        } else {
            Err(WorkerError::new(
                "guest trust or runner delivery does not match the runtime backend",
            ))
        }
    }
}
