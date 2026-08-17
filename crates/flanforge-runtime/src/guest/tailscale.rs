use std::{path::Path, process::Stdio};

use tokio::io::AsyncWriteExt;

use flanforge_manager::WorkerError;

use super::{GuestControl, ensure_ip, shell_quote};

/// One path for both the join and the retention logout: a strip that looked
/// somewhere else would silently bake the tailnet identity into the image.
pub(super) const TAILSCALE_PATH: &str = "/opt/homebrew/bin/tailscale";
const GUEST_KEY_PATH: &str = "/tmp/flanforged-tailscale-preauth-key";
const MAX_KEY_BYTES: u64 = 1_024;

impl GuestControl {
    pub(crate) async fn ensure_tailscale_connected(&self, ip: &str) -> Result<(), WorkerError> {
        ensure_ip(ip)?;
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
            login_server.as_str(),
            self.tailscale.hostname.as_deref(),
            &extra_args,
            &self.config.ssh_user,
        );
        let mut child = self
            .ssh_command(ip)
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| WorkerError::new("cannot start guest Tailscale bootstrap"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| WorkerError::new("cannot open guest Tailscale input"))?;
        stdin
            .write_all(key.as_bytes())
            .await
            .map_err(|_| WorkerError::new("cannot deliver guest Tailscale credential"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| WorkerError::new("cannot deliver guest Tailscale credential"))?;
        drop(stdin);
        let status = child
            .wait()
            .await
            .map_err(|_| WorkerError::new("cannot wait for guest Tailscale bootstrap"))?;
        if status.success() {
            tracing::info!("guest Tailscale connection is ready");
            Ok(())
        } else {
            tracing::warn!(
                exit_code = status.code(),
                "guest Tailscale bootstrap failed"
            );
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
    login_server: &str,
    hostname: Option<&str>,
    extra_args: &[String],
    operator: &str,
) -> String {
    // The operator is a per-profile preference, so the login that starts a new
    // profile must re-assert it or the guest account loses local API access.
    let mut arguments = vec![
        format!("--auth-key=file:{GUEST_KEY_PATH}"),
        format!("--login-server={login_server}"),
        format!("--operator={operator}"),
    ];
    if let Some(hostname) = hostname {
        arguments.push(format!("--hostname={hostname}"));
    }
    arguments.extend(extra_args.iter().cloned());
    let arguments = arguments
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let key_path = shell_quote(GUEST_KEY_PATH);
    let tailscale = shell_quote(TAILSCALE_PATH);
    format!(
        "umask 077; key_path={key_path}; cleanup() {{ /bin/rm -f \"$key_path\"; }}; on_signal() {{ cleanup; trap - EXIT; exit 1; }}; trap cleanup EXIT; trap on_signal HUP INT TERM; IFS= read -r key || exit 1; printf %s \"$key\" > \"$key_path\"; unset key; {tailscale} logout >/dev/null 2>&1 && {tailscale} up {arguments} >/dev/null && {tailscale} status --json | /usr/bin/grep -Eq '\"BackendState\"[[:space:]]*:[[:space:]]*\"Running\"'"
    )
}

#[cfg(test)]
mod tests;
