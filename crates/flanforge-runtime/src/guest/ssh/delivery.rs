use std::{path::Path, process::Stdio};

use flanforge_manager::WorkerError;

use super::SshChannel;
use crate::{
    channel::{
        GuestCapture, GuestChannel, GuestCommand, GuestExit, GuestFileTransfer, GuestProgram,
    },
    guest::{
        GuestSession,
        input::validate_absolute_path,
        paths::{RUNNER_BASE, RUNNER_TEMPLATE},
        shell_quote,
    },
};

/// The longest staging path the guest may report back. A longer reply is a
/// guest that is not answering the question asked, so it is refused unread.
const MAX_STAGING_REPLY_BYTES: usize = 256;

/// Room for the install step's own error text, kept so a failure names its
/// cause instead of a bare exit code.
const MAX_INSTALL_DIAGNOSTIC_BYTES: usize = 4_096;

#[async_trait::async_trait]
impl GuestFileTransfer for SshChannel {
    async fn install_file(
        &self,
        session: &GuestSession,
        source: &Path,
        destination: &Path,
    ) -> Result<(), WorkerError> {
        validate_absolute_path(&source.to_string_lossy())
            .map_err(|_| WorkerError::new("runner source path is structurally invalid"))?;
        validate_absolute_path(&destination.to_string_lossy())
            .map_err(|_| WorkerError::new("guest runner path is structurally invalid"))?;
        let temporary = self.allocate_staging_file(session).await?;
        let settings = self.settings_for(session);
        let address = session
            .address()
            .ok_or_else(|| WorkerError::new("guest session has no address for the SSH channel"))?;
        // scp reads the first unbracketed colon as the path separator, so a
        // bare IPv6 literal must be bracketed to survive.
        let target = if address.contains(':') {
            format!("{}@[{address}]:{temporary}", settings.user)
        } else {
            format!("{}@{address}:{temporary}", settings.user)
        };
        let status = self
            .scp_command(&settings)
            .arg(source)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| WorkerError::new("cannot copy Forgejo Runner into guest"))?;
        if !status.success() {
            self.remove_file(session, &temporary).await;
            tracing::warn!(
                exit_code = status.code(),
                "Forgejo Runner copy into guest failed"
            );
            return Err(WorkerError::new("cannot copy Forgejo Runner into guest"));
        }
        let staged = shell_quote(&temporary);
        let runner = shell_quote(&destination.to_string_lossy());
        // BSD install creates no parent directories, and the runner's home
        // layout is the image's business, not a delivery precondition.
        let parent = shell_quote(
            &destination
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .to_string_lossy(),
        );
        let script = format!(
            "{{ /bin/mkdir -p {parent} && /usr/bin/install -m 0700 {staged} {runner}; }} 2>&1; status=$?; /bin/rm -f {staged}; exit \"$status\""
        );
        let command = GuestCommand::new(
            GuestProgram::Script(&script),
            None,
            GuestCapture::Bounded(MAX_INSTALL_DIAGNOSTIC_BYTES),
        )?;
        let output = self
            .run(session, &command)
            .await
            .map_err(|_| WorkerError::new("cannot install guest runner"))?;
        if output.exit().is_success() {
            tracing::info!("Forgejo Runner staged in guest");
            Ok(())
        } else {
            let detail = String::from_utf8_lossy(output.stdout());
            tracing::warn!(
                exit_code = output.exit().code(),
                detail = %detail.trim(),
                "Forgejo Runner install in guest failed"
            );
            Err(WorkerError::new("cannot install guest runner"))
        }
    }
}

impl SshChannel {
    async fn allocate_staging_file(&self, session: &GuestSession) -> Result<String, WorkerError> {
        let script = format!(
            "umask 077; path=$(/usr/bin/mktemp {RUNNER_TEMPLATE}) || exit 1; /bin/chmod 0600 \"$path\" || exit 1; /usr/bin/printf '%s\\n' \"$path\""
        );
        let command = GuestCommand::new(
            GuestProgram::Script(&script),
            None,
            GuestCapture::Bounded(MAX_STAGING_REPLY_BYTES),
        )?;
        let output = self
            .run(session, &command)
            .await
            .map_err(|_| WorkerError::new("cannot reserve guest runner staging file"))?;
        if !output.exit().is_success() || output.is_truncated() {
            return Err(WorkerError::new("cannot reserve guest runner staging file"));
        }
        // The guest's login shell may print before the command runs, so the
        // path is the last non-empty line rather than the whole capture.
        let path = std::str::from_utf8(output.stdout())
            .map_err(|_| WorkerError::new("guest runner staging path is invalid"))?
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .ok_or_else(|| WorkerError::new("guest runner staging path is invalid"))?;
        let Some(suffix) = path.strip_prefix(&format!("{RUNNER_BASE}.")) else {
            return Err(WorkerError::new("guest runner staging path is invalid"));
        };
        if suffix.len() < 6
            || suffix.len() > 32
            || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(WorkerError::new("guest runner staging path is invalid"));
        }
        Ok(path.to_owned())
    }

    async fn remove_file(&self, session: &GuestSession, path: &str) {
        let _ = self
            .quiet(session, &format!("/bin/rm -f {}", shell_quote(path)))
            .await;
    }

    /// One discarded-output command, reduced to the exit every caller reads.
    async fn quiet(&self, session: &GuestSession, script: &str) -> Result<GuestExit, WorkerError> {
        let command = GuestCommand::quiet(script)?;
        Ok(self.run(session, &command).await?.exit())
    }
}
