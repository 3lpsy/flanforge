use std::path::Path;

use flanforge_forgejo::RunnerCredentials;
use flanforge_manager::WorkerError;
use validator::Validate;

use super::{
    GuestControl, GuestSession,
    input::{GuestRunnerInput, validate_absolute_path},
    script::runner_script,
    shell_quote,
};

/// Everything one one-job runner needs, so the channel-neutral half of the
/// start stays one argument rather than six positional ones.
#[derive(Clone, Copy, Debug)]
pub struct RunnerSpawn<'a> {
    pub server_url: &'a str,
    pub credentials: &'a RunnerCredentials,
    pub label: &'a str,
    pub handle: &'a str,
    /// Names the guest-side process for a channel that can end it independently
    /// of the daemon's own connection.
    pub allocation_id: uuid::Uuid,
    /// The budget after which the guest must end the job on its own, for the
    /// case where the daemon never comes back to end it.
    pub lifetime: std::time::Duration,
}
use crate::channel::{
    GuestCapture, GuestCommand, GuestProcess, GuestProgram, GuestSecret, GuestSpawn,
};

impl GuestControl {
    /// Copies the host's runner into the guest. Only a channel with a file
    /// transport can do this, so a channel without one refuses rather than
    /// silently leaving the guest without a runner.
    pub(crate) async fn stage_runner(
        &self,
        session: &GuestSession,
        source: &Path,
    ) -> Result<(), WorkerError> {
        let transfer = self.transfer.as_ref().ok_or_else(|| {
            WorkerError::new("the guest control channel cannot deliver a host-copied runner")
        })?;
        transfer
            .install_file(session, source, &self.runner_path)
            .await
    }

    pub(crate) async fn ensure_runner_available(
        &self,
        session: &GuestSession,
    ) -> Result<(), WorkerError> {
        let runner_path = self.runner_path.to_string_lossy();
        validate_absolute_path(&runner_path)
            .map_err(|_| WorkerError::new("guest runner path is structurally invalid"))?;
        let runner = shell_quote(&runner_path);
        let script = format!("test -x {runner} && {runner} one-job --help >/dev/null");
        let command = GuestCommand::quiet(&script)?;
        let exit = self
            .channel
            .run(session, &command)
            .await
            .map_err(|_| WorkerError::new("cannot verify image-installed guest runner"))?
            .exit();
        if exit.is_success() {
            tracing::info!("image-installed Forgejo Runner verified in guest");
            Ok(())
        } else {
            Err(WorkerError::new(
                "image-installed guest runner is missing or incompatible",
            ))
        }
    }

    /// Starts the one-job runner over one pinned session. The registration
    /// token reaches it on stdin, never in argv.
    ///
    /// # Errors
    /// Returns an error when the request is structurally invalid, or when the
    /// runner cannot be started on the guest.
    pub async fn spawn_session_runner(
        &self,
        session: &GuestSession,
        request: &RunnerSpawn<'_>,
    ) -> Result<Box<dyn GuestProcess>, WorkerError> {
        let RunnerSpawn {
            server_url,
            credentials,
            label,
            handle,
            allocation_id,
            lifetime,
        } = *request;
        credentials
            .validate()
            .map_err(|_| WorkerError::new("guest runner credential is structurally invalid"))?;
        let label = format!("{label}:host");
        GuestRunnerInput {
            runner_path: self.runner_path.to_string_lossy().into_owned(),
            server_url: server_url.to_owned(),
            uuid: credentials.uuid.clone(),
            label: label.clone(),
            handle: handle.to_owned(),
        }
        .validate()
        .map_err(|_| WorkerError::new("guest runner command is structurally invalid"))?;
        let script = runner_script(
            &self.runner_path.to_string_lossy(),
            server_url,
            &credentials.uuid,
            &label,
            handle,
        );
        let token = GuestSecret::line(&credentials.token)?;
        let command = GuestCommand::new(
            GuestProgram::Script(&script),
            Some(&token),
            GuestCapture::Discard,
        )?;
        let spawn = GuestSpawn::new(command, allocation_id, lifetime)?;
        let process = self
            .channel
            .spawn(session, &spawn)
            .await
            .map_err(|_| WorkerError::new("cannot start guest runner"))?;
        tracing::info!(runner_id = credentials.id, %label, "one-job guest runner started");
        Ok(process)
    }
}
