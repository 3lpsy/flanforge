use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flanforge_config::load_config;
use tokio::process::Command;

use super::paths::ServicePaths;

/// Prints the configured log file, falling back to what launchd captured.
pub(super) async fn logs(config_path: &Path, arguments: crate::cli::DaemonLogsArgs) -> Result<()> {
    let paths = ServicePaths::discover()?;
    let path = if arguments.stderr {
        paths.stderr_log.clone()
    } else {
        configured_log_path(config_path)
            .await
            .unwrap_or_else(|| paths.stdout_log.clone())
    };
    if !path.is_file() {
        bail!(
            "no log at {}; the service may not have run yet, or [logging].path is unset",
            path.display()
        );
    }
    eprintln!("==> {}", path.display());
    let mut command = Command::new("/usr/bin/tail");
    command.arg("-n").arg(arguments.lines.to_string());
    if arguments.follow {
        // -F reopens the path, so an external rotation does not end the follow.
        command.arg("-F");
    }
    let status = command
        .arg(&path)
        .status()
        .await
        .context("cannot invoke tail")?;
    if status.success() {
        Ok(())
    } else {
        bail!("tail exited with {status}")
    }
}

async fn configured_log_path(config_path: &Path) -> Option<PathBuf> {
    load_config(config_path).await.ok()?.logging.path
}
