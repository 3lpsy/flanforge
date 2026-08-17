use std::{
    path::{Component, Path},
    process::Stdio,
};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use super::{super::control::current_uid, firewall};

const SUDO: &str = "/usr/bin/sudo";

pub(super) async fn is_root() -> Result<bool> {
    Ok(current_uid().await? == 0)
}

/// Re-executes this same binary through sudo with the marker argument, so a
/// second elevation cannot be attempted from the child.
pub(super) async fn elevate(config_path: &Path, binary: &Path) -> Result<()> {
    let executable = std::env::current_exe().context("cannot locate current executable")?;
    println!("elevating with sudo to change the Application Firewall");
    let status = Command::new(SUDO)
        .arg("--")
        .arg(&executable)
        .arg("--config")
        .arg(config_path)
        .arg("daemon")
        .arg("priv")
        .arg("--elevated")
        .arg(binary)
        .stdin(Stdio::inherit())
        .kill_on_drop(true)
        .status()
        .await
        .context("cannot invoke sudo")?;
    if !status.success() {
        bail!("sudo did not complete the firewall change");
    }
    Ok(())
}

/// The elevated child: it applies only what needs root and reports nothing
/// else, leaving the gate report to the process that called it.
pub(super) async fn apply_elevated(binary: &Path) -> Result<()> {
    if !is_root().await? {
        bail!("--elevated must run as root; run the command without it instead");
    }
    ensure_authorizable(binary)?;
    firewall::ensure_allowed(binary).await?;
    println!("firewall:   added and unblocked {}", binary.display());
    Ok(())
}

/// The path is applied as root, so it is checked rather than trusted.
fn ensure_authorizable(binary: &Path) -> Result<()> {
    if !binary.is_absolute()
        || binary
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "{} must be an absolute path without parent traversal",
            binary.display()
        );
    }
    if !binary.is_file() {
        bail!("{} is not a file", binary.display());
    }
    Ok(())
}
