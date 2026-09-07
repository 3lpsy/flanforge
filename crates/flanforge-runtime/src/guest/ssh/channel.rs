use std::{path::PathBuf, process::Stdio, time::Duration};

use flanforge_core::GuestConfig;
use flanforge_manager::WorkerError;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Child,
};

use super::{options, process::SshProcess, settings::SshSettings};
use crate::{
    channel::{
        GuestCapture, GuestChannel, GuestCommand, GuestOutput, GuestProcess, GuestProgram,
        GuestSecret, GuestSpawn,
    },
    guest::{GuestSession, GuestTrust, shell_quote},
};

/// Drives a guest over an SSH client process. The client's lifetime is the kill
/// switch: closing the session fires the remote script's own trap.
#[derive(Debug)]
pub struct SshChannel {
    settings: SshSettings,
    /// The privileged automation account's identity over the same transport.
    privileged: SshSettings,
    pub(super) ssh_path: PathBuf,
    pub(super) scp_path: PathBuf,
}

impl SshChannel {
    /// # Errors
    /// Returns an error when the guest configuration has no `[guest.ssh]`
    /// table, which validation admits only for a channel that needs no SSH.
    pub fn new(
        config: &GuestConfig,
        ssh_path: PathBuf,
        scp_path: PathBuf,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            settings: SshSettings::from_config(config)?,
            privileged: SshSettings::privileged_from_config(config)?,
            ssh_path,
            scp_path,
        })
    }

    /// The daemon-owned option vector, before any per-session host-key pinning.
    #[must_use]
    pub fn base_arguments(&self) -> Vec<String> {
        options::base_arguments(&self.settings)
    }

    /// An allocation-pinned session carries the host key its seed injected, so
    /// it overrides the configured anchor rather than reading one.
    pub(super) fn settings_for(&self, session: &GuestSession) -> SshSettings {
        Self::resolved(&self.settings, session)
    }

    fn resolved(base: &SshSettings, session: &GuestSession) -> SshSettings {
        match session.trust() {
            GuestTrust::Configured => base.clone(),
            GuestTrust::Allocation {
                known_hosts_file,
                host_key_alias,
            } => base.pinned(known_hosts_file.clone(), host_key_alias.clone()),
        }
    }

    /// Spawns the client with the stdio every guest command uses: stderr always
    /// discarded, stdout kept only when asked, and stdin a pipe only when a
    /// secret has to reach it.
    fn start(
        &self,
        settings: &SshSettings,
        session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<Child, WorkerError> {
        let mut process = self.ssh_command(settings, &Self::address(session)?);
        process
            .arg(remote_command(command.program()))
            .stderr(Stdio::null())
            .stdin(if command.stdin().is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(match command.capture() {
                GuestCapture::Discard => Stdio::null(),
                GuestCapture::Bounded(_) => Stdio::piped(),
            });
        process
            .spawn()
            .map_err(|_| WorkerError::new("cannot start guest command"))
    }

    /// SSH carries commands over the network, so a session with no address is a
    /// guest this channel was never able to reach. Refusing here is the guard
    /// that keeps an agent-bound session from silently becoming an SSH one.
    fn address(session: &GuestSession) -> Result<String, WorkerError> {
        session
            .address()
            .ok_or_else(|| WorkerError::new("guest session has no address for the SSH channel"))
    }

    /// Delivers the one secret a guest command may take, then closes the pipe so
    /// the guest's `read` returns.
    async fn deliver(child: &mut Child, secret: Option<&GuestSecret>) -> Result<(), WorkerError> {
        let Some(secret) = secret else {
            return Ok(());
        };
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| WorkerError::new("cannot open guest command input"))?;
        stdin
            .write_all(secret.as_bytes())
            .await
            .map_err(|_| WorkerError::new("cannot deliver guest command input"))
    }
}

#[async_trait::async_trait]
impl GuestChannel for SshChannel {
    async fn ensure_ready(
        &self,
        session: &GuestSession,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        let settings = self.settings_for(session);
        let address = Self::address(session)?;
        tracing::debug!("waiting for guest SSH readiness");
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let status = self
                .ssh_command(&settings, &address)
                .arg("true")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            if status.is_ok_and(|status| status.success()) {
                tracing::info!("guest SSH is ready");
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("guest SSH did not become ready"));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn run(
        &self,
        session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        let settings = self.settings_for(session);
        self.run_resolved(&settings, session, command).await
    }

    async fn run_privileged(
        &self,
        session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        let settings = Self::resolved(&self.privileged, session);
        self.run_resolved(&settings, session, command).await
    }

    /// The spawn's handle and lifetime are deliberately unused: this channel
    /// names no guest-side process, and its own session lifetime plus the
    /// remote script's trap are the kill switch.
    async fn spawn(
        &self,
        session: &GuestSession,
        spawn: &GuestSpawn<'_>,
    ) -> Result<Box<dyn GuestProcess>, WorkerError> {
        let command = spawn.command();
        let settings = self.settings_for(session);
        let mut child = self.start(&settings, session, command)?;
        Self::deliver(&mut child, command.stdin()).await?;
        Ok(Box::new(SshProcess::new(child)))
    }
}

impl SshChannel {
    async fn run_resolved(
        &self,
        settings: &SshSettings,
        session: &GuestSession,
        command: &GuestCommand<'_>,
    ) -> Result<GuestOutput, WorkerError> {
        let mut child = self.start(settings, session, command)?;
        Self::deliver(&mut child, command.stdin()).await?;
        let GuestCapture::Bounded(limit) = command.capture() else {
            let status = child
                .wait()
                .await
                .map_err(|_| WorkerError::new("cannot wait for guest command"))?;
            return Ok(GuestOutput::new(status.into(), Vec::new(), false));
        };
        // Read one byte past the cap to detect truncation, then drain the
        // rest unbuffered: a guest cannot grow the daemon's memory with its
        // stdout, and the child is never blocked on a full pipe.
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| WorkerError::new("guest command has no stdout pipe"))?;
        let mut stdout = Vec::new();
        (&mut stdout_pipe)
            .take(limit as u64 + 1)
            .read_to_end(&mut stdout)
            .await
            .map_err(|_| WorkerError::new("cannot read guest command output"))?;
        let _ = tokio::io::copy(&mut stdout_pipe, &mut tokio::io::sink()).await;
        let status = child
            .wait()
            .await
            .map_err(|_| WorkerError::new("cannot wait for guest command"))?;
        let is_truncated = stdout.len() > limit;
        stdout.truncate(limit);
        Ok(GuestOutput::new(status.into(), stdout, is_truncated))
    }
}

/// SSH takes one remote command line, so a program form is joined into one
/// shell-quoted word list rather than losing its argv.
fn remote_command(program: GuestProgram<'_>) -> String {
    match program {
        GuestProgram::Script(script) => script.to_owned(),
        GuestProgram::Program { path, arguments } => std::iter::once(path)
            .map(shell_quote)
            .chain(arguments.iter().map(|argument| shell_quote(argument)))
            .collect::<Vec<_>>()
            .join(" "),
    }
}
