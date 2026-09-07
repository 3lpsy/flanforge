use std::path::Path;

use flanforge_core::RuntimeBackendKind;
use flanforge_manager::WorkerError;

use super::{
    GuestControl, GuestSession, paths::TAILSCALE_KEY_TEMPLATE as GUEST_KEY_TEMPLATE, shell_quote,
};
use crate::channel::{GuestCapture, GuestCommand, GuestProgram, GuestSecret};

/// Each guest platform has one fixed path for its connection bootstrap.
pub const TART_TAILSCALE_PATH: &str = "/opt/homebrew/bin/tailscale";
pub const LIBVIRT_TAILSCALE_PATH: &str = "/usr/bin/tailscale";
const MAX_KEY_BYTES: u64 = 1_024;

impl GuestControl {
    pub(crate) async fn ensure_tailscale_connected(
        &self,
        session: &GuestSession,
    ) -> Result<(), WorkerError> {
        if !self.tailscale.enabled {
            tracing::trace!("guest Tailscale bootstrap is disabled");
            return Ok(());
        }
        let key_path = self
            .tailscale
            .preauth_key_file
            .as_deref()
            .ok_or_else(|| WorkerError::new("Tailscale preauth key file is not configured"))?;
        let login_server = self
            .tailscale
            .login_server
            .as_ref()
            .ok_or_else(|| WorkerError::new("Tailscale login server is not configured"))?;
        let key = read_preauth_key(key_path).await?;
        let extra_args = self
            .tailscale
            .extra_arguments()
            .map_err(|_| WorkerError::new("Tailscale extra arguments are structurally invalid"))?;
        let script = tailscale_script(
            self.tailscale_path,
            login_server.as_str(),
            self.tailscale.hostname.as_deref(),
            &extra_args,
            &self.runner_user,
        );
        let secret = GuestSecret::line(&key)?;
        let command = GuestCommand::new(
            GuestProgram::Script(&script),
            Some(&secret),
            GuestCapture::Discard,
        )?;
        let exit = self
            .channel
            .run(session, &command)
            .await
            .map_err(|_| WorkerError::new("cannot start guest Tailscale bootstrap"))?
            .exit();
        if exit.is_success() {
            tracing::info!("guest Tailscale connection is ready");
            Ok(())
        } else {
            tracing::warn!(exit_code = exit.code(), "guest Tailscale bootstrap failed");
            Err(WorkerError::new("guest Tailscale bootstrap failed"))
        }
    }
}

async fn read_preauth_key(path: &Path) -> Result<String, WorkerError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| WorkerError::new("cannot inspect Tailscale preauth key file"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_KEY_BYTES {
        return Err(WorkerError::new("Tailscale preauth key file is invalid"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(WorkerError::new(
                "Tailscale preauth key file permissions are not private",
            ));
        }
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| WorkerError::new("cannot read Tailscale preauth key file"))?;
    let raw = std::str::from_utf8(&bytes)
        .map_err(|_| WorkerError::new("Tailscale preauth key is not UTF-8"))?;
    let key = raw.trim_end_matches(['\r', '\n']);
    if !(8..=512).contains(&key.len())
        || key
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || !byte.is_ascii_graphic())
    {
        return Err(WorkerError::new(
            "Tailscale preauth key is structurally invalid",
        ));
    }
    Ok(key.to_owned())
}

fn tailscale_script(
    tailscale_path: &str,
    login_server: &str,
    hostname: Option<&str>,
    extra_args: &[String],
    operator: &str,
) -> String {
    // The operator is a per-profile preference, so the login that starts a new
    // profile must re-assert it or the guest account loses local API access.
    let mut arguments = extra_args.to_vec();
    arguments.extend([
        format!("--login-server={login_server}"),
        format!("--operator={operator}"),
    ]);
    if let Some(hostname) = hostname {
        arguments.push(format!("--hostname={hostname}"));
    }
    let arguments = arguments
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let key_template = shell_quote(GUEST_KEY_TEMPLATE);
    let tailscale = shell_quote(tailscale_path);
    format!(
        "umask 077; key_path=$(/usr/bin/mktemp {key_template}) || exit 1; /bin/chmod 0600 \"$key_path\" || {{ /bin/rm -f \"$key_path\"; exit 1; }}; cleanup() {{ /bin/rm -f \"$key_path\"; }}; on_signal() {{ cleanup; trap - EXIT; exit 1; }}; trap cleanup EXIT; trap on_signal HUP INT TERM; IFS= read -r key || exit 1; /usr/bin/printf %s \"$key\" > \"$key_path\" || exit 1; unset key; {tailscale} logout >/dev/null 2>&1 && {tailscale} up {arguments} \"--auth-key=file:$key_path\" >/dev/null && {tailscale} status --json | /usr/bin/grep -Eq '\"BackendState\"[[:space:]]*:[[:space:]]*\"Running\"'"
    )
}

pub(super) const fn path(backend: RuntimeBackendKind) -> &'static str {
    match backend {
        RuntimeBackendKind::Tart => TART_TAILSCALE_PATH,
        RuntimeBackendKind::Libvirt => LIBVIRT_TAILSCALE_PATH,
    }
}

#[cfg(test)]
mod tests;
