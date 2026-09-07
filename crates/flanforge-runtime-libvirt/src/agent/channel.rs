use std::time::Duration;

use flanforge_libvirt_wire::{AgentExecOutcome, OwnershipManifest};
use flanforge_manager::WorkerError;
use flanforge_runtime::{
    GuestCapture, GuestChannel, GuestCommand, GuestExit, GuestOutput, GuestProcess, GuestSession,
    GuestSpawn,
};

use crate::actor::LibvirtActor;

use super::{
    exec::{JobAccount, request, transient_unit},
    process::AgentProcess,
};

/// The helper clamps its own agent call to five seconds, and `guest-exec`
/// returns a pid immediately, so nothing here waits on a round trip.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

const MIN_POLL: Duration = Duration::from_secs(1);
const MAX_POLL: Duration = Duration::from_secs(15);

/// Drives one guest over its QEMU guest agent. The channel is virtio-serial and
/// host-local to the hypervisor, so it reaches a guest the daemon has no route
/// to — which is the whole reason it exists.
///
/// Unlike SSH there is no connection whose lifetime is a kill switch: a command
/// abandoned by its caller keeps running in the guest until the domain is
/// destroyed. Supervised work therefore goes through `spawn`, which names a
/// transient unit PID 1 can end on its own.
#[derive(Clone, Debug)]
pub(crate) struct AgentChannel {
    actor: LibvirtActor,
    manifest: OwnershipManifest,
    account: JobAccount,
    /// The privileged automation account, or the named reason the base
    /// provides none — surfaced only when a privileged command is asked for.
    privileged: Result<JobAccount, String>,
}

impl AgentChannel {
    pub(crate) const fn new(
        actor: LibvirtActor,
        manifest: OwnershipManifest,
        account: JobAccount,
        privileged: Result<JobAccount, String>,
    ) -> Self {
        Self {
            actor,
            manifest,
            account,
            privileged,
        }
    }

    /// Runs one already-built request to completion, polling its status on a
    /// widening interval because each probe costs a helper process.
    async fn complete(
        &self,
        exec: flanforge_libvirt_wire::AgentExecRequest,
        capture: GuestCapture,
    ) -> Result<GuestOutput, WorkerError> {
        let pid = self
            .actor
            .agent_exec(self.manifest.clone(), exec, CALL_TIMEOUT)
            .await?;
        let mut interval = MIN_POLL;
        loop {
            tokio::time::sleep(interval).await;
            match self
                .actor
                .agent_exec_status(self.manifest.clone(), pid, CALL_TIMEOUT)
                .await
            {
                Ok(AgentExecOutcome::Running) => interval = (interval * 2).min(MAX_POLL),
                Ok(AgentExecOutcome::Exited {
                    exit_code,
                    signal,
                    stdout,
                    is_truncated,
                    stderr,
                }) => {
                    let exit = exit_code
                        .map(GuestExit::Code)
                        .or_else(|| signal.map(GuestExit::Signal))
                        .ok_or_else(|| WorkerError::new("guest command reported no verdict"))?;
                    return Ok(bounded(exit, stdout, is_truncated, stderr, capture));
                }
                Ok(AgentExecOutcome::Lost { reason }) => {
                    tracing::warn!(%reason, "guest agent lost a daemon-owned command");
                    return Err(WorkerError::new("guest command became unobservable"));
                }
                Err(error) if error.is_retryable() => {
                    interval = (interval * 2).min(MAX_POLL);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[async_trait::async_trait]
impl GuestChannel for AgentChannel {
    /// Reachability only. Whether first-boot provisioning finished is a
    /// property of the image, and its gate belongs to the backend that baked
    /// it — so `channel = "ssh"` gets the same diagnostics.
    async fn ensure_ready(
        &self,
        _session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut interval = MIN_POLL;
        loop {
            match self
                .actor
                .agent_probe(self.manifest.clone(), CALL_TIMEOUT)
                .await
            {
                Ok(true) => {
                    tracing::info!("guest agent channel is ready");
                    return Ok(());
                }
                // A blocked RPC never becomes unblocked by waiting, so this
                // fails now instead of burning the whole boot budget.
                Ok(false) => {
                    return Err(WorkerError::new(
                        "guest agent channel: this guest blocks guest-exec; publish a base image \
                         with the RPC enabled, or drive this backend over SSH",
                    ));
                }
                Err(error) if error.is_retryable() => {
                    tracing::debug!(%error, "waiting for the guest agent");
                }
                Err(error) => return Err(error.into()),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("guest agent did not become ready"));
            }
            tokio::time::sleep(interval).await;
            interval = (interval * 2).min(MAX_POLL);
        }
    }

    async fn run(
        &self,
        _session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        let input = command.stdin().map(|secret| secret.as_bytes().to_vec());
        let exec = request(
            &self.account,
            command.program(),
            input,
            matches!(command.capture(), GuestCapture::Bounded(_)),
        );
        self.complete(exec, command.capture()).await
    }

    /// The same login-shell shape as `run`, dropped to the privileged account
    /// instead; its sudo rule is what turns the session into root-capable.
    async fn run_privileged(
        &self,
        _session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        let account = self
            .privileged
            .as_ref()
            .map_err(|reason| WorkerError::new(reason.clone()))?;
        let input = command.stdin().map(|secret| secret.as_bytes().to_vec());
        let exec = request(
            account,
            command.program(),
            input,
            matches!(command.capture(), GuestCapture::Bounded(_)),
        );
        self.complete(exec, command.capture()).await
    }

    async fn spawn(
        &self,
        _session: &GuestSession,
        spawn: &GuestSpawn<'_>,
    ) -> Result<Box<dyn GuestProcess>, WorkerError> {
        let command = spawn.command();
        let flanforge_runtime::GuestProgram::Script(script) = command.program() else {
            return Err(WorkerError::new(
                "a supervised guest command must be a script the job account can run",
            ));
        };
        let exec = transient_unit(
            &self.account,
            script,
            spawn.handle(),
            spawn.lifetime(),
            command.stdin().map(|secret| secret.as_bytes().to_vec()),
        );
        let pid = self
            .actor
            .agent_exec(self.manifest.clone(), exec, CALL_TIMEOUT)
            .await?;
        Ok(Box::new(AgentProcess::new(
            self.actor.clone(),
            self.manifest.clone(),
            spawn.handle(),
            pid,
            CALL_TIMEOUT,
        )))
    }
}

/// Re-bounds the guest's own output to what the caller asked for, so a helper
/// that widened its cap could not widen what a caller keeps.
fn bounded(
    exit: GuestExit,
    mut stdout: Vec<u8>,
    is_truncated: bool,
    mut stderr: Vec<u8>,
    capture: GuestCapture,
) -> GuestOutput {
    match capture {
        GuestCapture::Discard => GuestOutput::new(exit, Vec::new(), false),
        GuestCapture::Bounded(limit) => {
            let is_truncated = is_truncated || stdout.len() > limit;
            stdout.truncate(limit);
            stderr.truncate(limit);
            GuestOutput::new(exit, stdout, is_truncated).with_stderr(stderr)
        }
    }
}
