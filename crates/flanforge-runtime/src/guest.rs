use std::{process::Stdio, time::Duration};

mod input;
mod known_hosts;
mod options;
mod script;
mod strip;
mod tailscale;

use flanforge_core::{GuestConfig, TailscaleConfig};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

use flanforge_forgejo::RunnerCredentials;
use flanforge_manager::WorkerError;
use input::{GuestRunnerInput, ensure_ip, validate_absolute_path};
pub use known_hosts::ensure_guest_known_hosts;
use script::runner_script;
#[cfg(test)]
pub(super) use script::runner_script_with_token_path;
pub(super) use script::shell_quote;
#[cfg(test)]
pub(super) use strip::{REGENERATION_SENTINEL, strip_script};
use validator::Validate;

#[derive(Clone, Debug)]
pub(super) struct GuestControl {
    config: GuestConfig,
    tailscale: TailscaleConfig,
    ssh_path: std::path::PathBuf,
    scp_path: std::path::PathBuf,
}

impl GuestControl {
    pub(super) fn new(
        config: GuestConfig,
        tailscale: TailscaleConfig,
        ssh_path: std::path::PathBuf,
        scp_path: std::path::PathBuf,
    ) -> Self {
        Self {
            config,
            tailscale,
            ssh_path,
            scp_path,
        }
    }

    pub(super) async fn wait_ready(&self, ip: &str, timeout: Duration) -> Result<(), WorkerError> {
        ensure_ip(ip)?;
        tracing::debug!("waiting for guest SSH readiness");
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let status = self
                .ssh_command(ip)
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
                return Err(WorkerError::new("Tart guest SSH did not become ready"));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub(super) async fn stage_runner(
        &self,
        ip: &str,
        source: &std::path::Path,
    ) -> Result<(), WorkerError> {
        ensure_ip(ip)?;
        validate_absolute_path(&source.to_string_lossy())
            .map_err(|_| WorkerError::new("runner source path is structurally invalid"))?;
        validate_absolute_path(&self.config.forgejo_runner_path.to_string_lossy())
            .map_err(|_| WorkerError::new("guest runner path is structurally invalid"))?;
        let destination = format!(
            "{}@{ip}:/tmp/flanforged-forgejo-runner",
            self.config.ssh_user
        );
        let status = self
            .scp_command()
            .arg(source)
            .arg(destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| WorkerError::new("cannot copy Forgejo Runner into guest"))?;
        if !status.success() {
            tracing::warn!(
                exit_code = status.code(),
                "Forgejo Runner copy into guest failed"
            );
            return Err(WorkerError::new("cannot copy Forgejo Runner into guest"));
        }
        let script = format!(
            "/usr/bin/install -m 0700 /tmp/flanforged-forgejo-runner {} && /bin/rm -f /tmp/flanforged-forgejo-runner",
            shell_quote(&self.config.forgejo_runner_path.to_string_lossy())
        );
        let status = self
            .ssh_command(ip)
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| WorkerError::new("cannot install guest runner"))?;
        if status.success() {
            tracing::info!("Forgejo Runner staged in guest");
            Ok(())
        } else {
            tracing::warn!(
                exit_code = status.code(),
                "Forgejo Runner install in guest failed"
            );
            Err(WorkerError::new("cannot install guest runner"))
        }
    }

    pub(super) async fn spawn_runner(
        &self,
        ip: &str,
        server_url: &str,
        credentials: &RunnerCredentials,
        label: &str,
        handle: &str,
    ) -> Result<Child, WorkerError> {
        ensure_ip(ip)?;
        credentials
            .validate()
            .map_err(|_| WorkerError::new("guest runner credential is structurally invalid"))?;
        let label = format!("{label}:host");
        GuestRunnerInput {
            runner_path: self
                .config
                .forgejo_runner_path
                .to_string_lossy()
                .into_owned(),
            server_url: server_url.to_owned(),
            uuid: credentials.uuid.clone(),
            label: label.clone(),
            handle: handle.to_owned(),
        }
        .validate()
        .map_err(|_| WorkerError::new("guest runner command is structurally invalid"))?;
        let script = runner_script(
            &self.config.forgejo_runner_path.to_string_lossy(),
            server_url,
            &credentials.uuid,
            &label,
            handle,
        );
        let mut child = self
            .ssh_command(ip)
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| WorkerError::new("cannot start guest runner"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| WorkerError::new("cannot open guest runner input"))?;
        stdin
            .write_all(credentials.token.as_bytes())
            .await
            .map_err(|_| WorkerError::new("cannot deliver guest runner credential"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| WorkerError::new("cannot deliver guest runner credential"))?;
        drop(stdin);
        tracing::info!(runner_id = credentials.id, %label, "one-job guest runner started");
        Ok(child)
    }

    fn ssh_command(&self, ip: &str) -> Command {
        let mut command = Command::new(&self.ssh_path);
        command.kill_on_drop(true);
        command.args(self.connection_arguments(ip));
        command
    }

    fn scp_command(&self) -> Command {
        let mut command = Command::new(&self.scp_path);
        command.kill_on_drop(true);
        command.args(self.base_arguments());
        command
    }

    fn connection_arguments(&self, ip: &str) -> Vec<String> {
        let mut arguments = self.base_arguments();
        arguments.push(format!("{}@{ip}", self.config.ssh_user));
        arguments
    }

    pub(super) fn base_arguments(&self) -> Vec<String> {
        options::base_arguments(&self.config)
    }
}
