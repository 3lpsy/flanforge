use std::time::Duration;

use flanforge_libvirt_wire::{AgentExecOutcome, OwnershipManifest};
use flanforge_manager::WorkerError;
use flanforge_runtime::{GuestExit, GuestProcess};
use uuid::Uuid;

use crate::actor::LibvirtActor;

use super::exec::{stop_unit, unit_name};

/// Each probe is one libvirt helper process and one agent round trip — an SSH
/// handshake per probe on `qemu+ssh` — so a job that may run for hours cannot be
/// probed at the Forgejo poll cadence.
const MIN_PROBE_INTERVAL: Duration = Duration::from_secs(1);
const MAX_PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// How many non-retryable status failures in a row mean the process can no
/// longer be reasoned about. `Lost` is never a verdict, so concluding it fails
/// supervision rather than deciding the job.
const MAX_OBSERVATION_FAILURES: u32 = 3;

/// How long a forced stop is followed up before the outcome is only logged.
/// `TimeoutStopSec` bounds PID 1's own escalation to SIGKILL well inside this.
const STOP_FOLLOW_UP: Duration = Duration::from_secs(15);

/// One guest-side job, named by the transient unit PID 1 owns rather than by
/// any connection the daemon holds.
#[derive(Debug)]
pub(crate) struct AgentProcess {
    actor: LibvirtActor,
    manifest: OwnershipManifest,
    handle: Uuid,
    pid: i64,
    call_timeout: Duration,
    next_probe_at: tokio::time::Instant,
    interval: Duration,
    failures: u32,
    /// The agent frees its exec record on the first reply reporting `exited`,
    /// so a terminal outcome is read exactly once and answered from here after.
    terminal: Option<GuestExit>,
}

impl AgentProcess {
    pub(super) fn new(
        actor: LibvirtActor,
        manifest: OwnershipManifest,
        handle: Uuid,
        pid: i64,
        call_timeout: Duration,
    ) -> Self {
        Self {
            actor,
            manifest,
            handle,
            pid,
            call_timeout,
            next_probe_at: tokio::time::Instant::now() + MIN_PROBE_INTERVAL,
            interval: MIN_PROBE_INTERVAL,
            failures: 0,
            terminal: None,
        }
    }

    fn back_off(&mut self) {
        self.interval = (self.interval * 2).min(MAX_PROBE_INTERVAL);
        self.next_probe_at = tokio::time::Instant::now() + self.interval;
    }

    /// The agent frees its exec record on the first `exited` reply, so the
    /// verdict is kept here and every later probe is answered without I/O.
    fn record(&mut self, exit: GuestExit) -> GuestExit {
        self.terminal = Some(exit);
        exit
    }
}

#[async_trait::async_trait]
impl GuestProcess for AgentProcess {
    async fn try_exit(&mut self) -> Result<Option<GuestExit>, WorkerError> {
        if let Some(exit) = self.terminal {
            return Ok(Some(exit));
        }
        if tokio::time::Instant::now() < self.next_probe_at {
            return Ok(None);
        }
        let outcome = self
            .actor
            .agent_exec_status(self.manifest.clone(), self.pid, self.call_timeout)
            .await;
        match outcome {
            Ok(AgentExecOutcome::Running) => {
                self.failures = 0;
                self.back_off();
                Ok(None)
            }
            Ok(AgentExecOutcome::Exited {
                exit_code, signal, ..
            }) => Ok(exit_code
                .map(GuestExit::Code)
                .or_else(|| signal.map(GuestExit::Signal))
                .map(|exit| self.record(exit))),
            Ok(AgentExecOutcome::Lost { reason }) => {
                tracing::warn!(%reason, unit = %unit_name(self.handle), "guest agent lost the job process");
                Ok(Some(self.record(GuestExit::Lost)))
            }
            Err(error) if error.is_retryable() => {
                tracing::debug!(%error, "guest agent status probe deferred");
                self.back_off();
                Ok(None)
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                tracing::warn!(%error, failures = self.failures, "guest agent status probe failed");
                if self.failures >= MAX_OBSERVATION_FAILURES {
                    return Ok(Some(self.record(GuestExit::Lost)));
                }
                self.back_off();
                Ok(None)
            }
        }
    }

    /// Stops the unit rather than the process: `systemctl stop` reaches the
    /// whole cgroup, so nothing the job started outlives it.
    async fn ensure_stopped(&mut self) {
        let unit = unit_name(self.handle);
        let stopped = self
            .actor
            .agent_exec(
                self.manifest.clone(),
                stop_unit(self.handle),
                self.call_timeout,
            )
            .await;
        match stopped {
            Ok(pid) => {
                tracing::info!(%unit, %pid, "stopping the guest job unit");
                self.await_stop(pid).await;
            }
            Err(error) => {
                tracing::warn!(%unit, %error, "cannot stop the guest job unit; the domain teardown is the fallback");
            }
        }
    }

    fn probe_now(&mut self) {
        self.interval = MIN_PROBE_INTERVAL;
        self.next_probe_at = tokio::time::Instant::now();
    }
}

impl AgentProcess {
    /// Follows the stop far enough to report it. Teardown never fails the
    /// caller: destroying the domain is the kill switch that needs no guest.
    async fn await_stop(&self, pid: i64) {
        let deadline = tokio::time::Instant::now() + STOP_FOLLOW_UP;
        while tokio::time::Instant::now() < deadline {
            match self
                .actor
                .agent_exec_status(self.manifest.clone(), pid, self.call_timeout)
                .await
            {
                Ok(AgentExecOutcome::Running) => {}
                Ok(AgentExecOutcome::Exited { exit_code, .. }) => {
                    tracing::info!(exit_code, "the guest job unit stopped");
                    return;
                }
                Ok(AgentExecOutcome::Lost { .. }) | Err(_) => return,
            }
            tokio::time::sleep(MIN_PROBE_INTERVAL).await;
        }
        tracing::warn!("the guest job unit did not confirm its stop in time");
    }
}
