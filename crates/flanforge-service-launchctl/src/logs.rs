use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flanforge_config::load_config;
use flanforge_service::LogOptions;
use tokio::process::Command;

use crate::{
    constants::TAIL_PATH,
    paths::{discover, is_present},
};

pub(crate) async fn logs(config_path: &Path, options: LogOptions) -> Result<()> {
    let paths = discover()?;
    let path = if options.is_stderr() {
        paths.stderr_log
    } else {
        configured_log_path(config_path)
            .await?
            .unwrap_or(paths.stdout_log)
    };
    if !is_present(&path).await? {
        bail!(
            "no log at {}; the service may not have run yet",
            path.display()
        );
    }
    eprintln!("==> {}", path.display());
    let mut command = Command::new(TAIL_PATH);
    command.arg("-n").arg(options.lines().to_string());
    if options.is_following() {
        command.arg("-F");
    }
    let status = command
        .arg(&path)
        .kill_on_drop(true)
        .status()
        .await
        .context("cannot invoke tail")?;
    if status.success() {
        Ok(())
    } else {
        bail!("tail exited with {status}")
    }
}

async fn configured_log_path(config_path: &Path) -> Result<Option<PathBuf>> {
    Ok(load_config(config_path).await?.logging.path)
}
