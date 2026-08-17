use std::path::Path;

use anyhow::{Result, bail};

use crate::cli::DaemonCommand;

use super::{control, install::install, logs::logs, privileges::priv_gates, status::status};

/// Installs or controls the per-user macOS daemon.
///
/// # Errors
///
/// Returns an error outside macOS, for an invalid service configuration, or
/// when installation or `launchctl` fails.
pub async fn run_daemon_command(path: &Path, command: DaemonCommand) -> Result<()> {
    ensure_macos()?;
    match command {
        DaemonCommand::Install => install(path).await,
        DaemonCommand::Start => control::start().await,
        DaemonCommand::Stop => control::stop().await,
        DaemonCommand::Restart => control::restart().await,
        DaemonCommand::Status => status(path).await,
        DaemonCommand::Logs(arguments) => logs(path, arguments).await,
        DaemonCommand::Priv(arguments) => priv_gates(path, arguments).await,
        DaemonCommand::Run => bail!("daemon run must execute in the foreground"),
    }
}

fn ensure_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("daemon service commands are supported only on macOS")
    }
}
