use std::process::Stdio;

use flanforge_manager::WorkerError;

use super::{GuestControl, ensure_ip, tailscale::TAILSCALE_PATH};

/// The workflow's own final step writes this; the daemon reads it as the only
/// evidence that the build the image is made of actually succeeded.
pub(crate) const REGENERATION_SENTINEL: &str = ".flanforge-regeneration-complete";

/// Removed best effort: `tailscale logout` is what revokes the identity, and a
/// root-owned state directory must not make retention impossible.
const TAILSCALE_STATE_PATHS: [&str; 2] = ["/opt/homebrew/var/lib/tailscale", "/Library/Tailscale"];

/// Everything the daemon knows a job can leave behind, relative to the runner
/// home. Caches are deliberately absent: they are the point of the image.
const HOME_PATHS: [&str; 12] = [
    ".cache/act",
    "_work",
    ".runner",
    ".gitconfig",
    ".git-credentials",
    ".config/git",
    ".netrc",
    ".ssh/known_hosts",
    ".bash_history",
    ".zsh_history",
    ".sh_history",
    ".zsh_sessions",
];

/// The daemon's own guest temporary files.
const TEMPORARY_PATHS: [&str; 3] = [
    "/tmp/flanforged-one-job-token",
    "/tmp/flanforged-tailscale-preauth-key",
    "/tmp/flanforged-forgejo-runner",
];

impl GuestControl {
    /// Verifies the workflow's success sentinel, then removes every credential
    /// class the daemon knows about and proves the removal.
    pub(crate) async fn ensure_stripped(&self, ip: &str) -> Result<(), WorkerError> {
        ensure_ip(ip)?;
        let status = self
            .ssh_command(ip)
            .arg(strip_script())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| WorkerError::new("cannot strip the guest before retention"))?;
        if status.success() {
            tracing::info!("guest credentials stripped and verified");
            Ok(())
        } else {
            tracing::warn!(
                exit_code = status.code(),
                "guest credential strip did not verify"
            );
            Err(WorkerError::new("guest credential strip did not verify"))
        }
    }
}

/// Fixed and daemon-owned: no workflow value is interpolated, and the guest's
/// SSH host keys, `authorized_keys`, and staged runner binary are untouched.
pub(crate) fn strip_script() -> String {
    let home = HOME_PATHS
        .iter()
        .map(|path| super::shell_quote(path))
        .collect::<Vec<_>>()
        .join(" ");
    let temporary = TEMPORARY_PATHS
        .iter()
        .map(|path| super::shell_quote(path))
        .collect::<Vec<_>>()
        .join(" ");
    let state = TAILSCALE_STATE_PATHS
        .iter()
        .map(|path| super::shell_quote(path))
        .collect::<Vec<_>>()
        .join(" ");
    let tailscale = super::shell_quote(TAILSCALE_PATH);
    let sentinel = super::shell_quote(REGENERATION_SENTINEL);
    format!(
        "set -u; sentinel=\"$HOME/\"{sentinel}; \
         if [ ! -f \"$sentinel\" ]; then exit 10; fi; \
         if [ -x {tailscale} ]; then \
           if ! {tailscale} logout >/dev/null 2>&1; then exit 11; fi; \
           /bin/rm -rf {state} >/dev/null 2>&1 || true; \
         fi; \
         for relative in {home}; do /bin/rm -rf \"$HOME/$relative\"; done; \
         /bin/rm -rf {temporary} \"$sentinel\"; \
         for relative in {home}; do \
           if [ -e \"$HOME/$relative\" ]; then exit 12; fi; \
         done; \
         for absolute in {temporary} \"$sentinel\"; do \
           if [ -e \"$absolute\" ]; then exit 12; fi; \
         done; \
         if [ -x {tailscale} ] && {tailscale} status --json 2>/dev/null | \
           /usr/bin/grep -Eq '\"BackendState\"[[:space:]]*:[[:space:]]*\"Running\"'; then exit 13; fi; \
         exit 0"
    )
}
