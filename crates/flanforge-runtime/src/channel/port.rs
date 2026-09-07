use std::{path::Path, time::Duration};

use flanforge_manager::WorkerError;

use super::{GuestCommand, GuestExit, GuestOutput, GuestSpawn};
use crate::guest::GuestSession;

/// The one way the daemon reaches a guest. Every backend-neutral guest step is
/// expressed here, so a backend can supply a transport without SSH.
#[async_trait::async_trait]
pub trait GuestChannel: std::fmt::Debug + Send + Sync {
    /// Returns once the channel can carry commands.
    ///
    /// # Errors
    /// Returns an error when the channel is unusable, or does not become usable
    /// before the timeout.
    async fn ensure_ready(
        &self,
        session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError>;

    /// Runs one bounded, daemon-owned command to completion. A non-zero guest
    /// exit is `Ok`, not an error: only the caller knows what it means.
    ///
    /// # Errors
    /// Returns an error when the command is structurally invalid or the channel
    /// cannot carry it.
    async fn run(
        &self,
        session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError>;

    /// Starts one long-running script and returns the handle supervision polls.
    ///
    /// # Errors
    /// Returns an error when the command is invalid or cannot be started.
    async fn spawn(
        &self,
        session: &GuestSession,
        spawn: &GuestSpawn<'_>,
    ) -> Result<Box<dyn GuestProcess>, WorkerError>;

    /// Runs one bounded, daemon-owned command as the guest's privileged
    /// automation account rather than the job account.
    ///
    /// # Errors
    /// Returns an error when the channel has no privileged path, the base
    /// bakes no such account, or the channel cannot carry the command.
    async fn run_privileged(
        &self,
        _session: &GuestSession,
        _command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        Err(WorkerError::new(
            "this guest channel has no privileged execution path",
        ))
    }
}

/// One started guest process, from the daemon's side.
#[async_trait::async_trait]
pub trait GuestProcess: std::fmt::Debug + Send + Sync {
    /// The exit once finished, or `None` while running — or while the channel
    /// is between probes. A channel that pays per probe rate-limits here.
    ///
    /// # Errors
    /// Returns an error only when the process can no longer be reasoned about;
    /// a lost observation is `Ok(None)`.
    async fn try_exit(&mut self) -> Result<Option<GuestExit>, WorkerError>;

    /// Ends the process and everything it started. Idempotent; never fails the
    /// caller — teardown reports through the log.
    async fn ensure_stopped(&mut self);

    /// Asks the next `try_exit` to probe without waiting for its schedule.
    fn probe_now(&mut self);
}

/// Host-to-guest file delivery. Only `RunnerDelivery::HostCopy` needs it and
/// only the SSH channel can provide it, so a channel without a file transport
/// simply never implements it.
#[async_trait::async_trait]
pub trait GuestFileTransfer: std::fmt::Debug + Send + Sync {
    /// Copies `source` into the guest and installs it, executable, at
    /// `destination`. The staging path is the daemon's own, and is removed
    /// whether or not the install succeeds.
    ///
    /// # Errors
    /// Returns an error for an unsafe path, or when copy or install fails.
    async fn install_file(
        &self,
        session: &GuestSession,
        source: &Path,
        destination: &Path,
    ) -> Result<(), WorkerError>;
}
