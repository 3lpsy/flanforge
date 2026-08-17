use std::{ffi::OsString, process::Stdio};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use super::paths::{SERVICE_LABEL, ServicePaths};

pub(super) async fn start() -> Result<()> {
    let paths = ServicePaths::discover()?;
    if !paths.launch_agent.is_file() {
        bail!("LaunchAgent is not installed; run `flanforged daemon install`");
    }
    let (domain, target) = launchd_identity().await?;
    if launchctl_status([
        "bootstrap".into(),
        domain,
        paths.launch_agent.as_os_str().to_owned(),
    ])
    .await
    .is_err()
    {
        tracing::debug!(
            service = SERVICE_LABEL,
            "LaunchAgent was already loaded or could not be bootstrapped; trying kickstart"
        );
    }
    launchctl(["enable".into(), target.clone()]).await?;
    launchctl(["kickstart".into(), target.clone()]).await?;
    ensure_service_running(&target).await?;
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent started");
    Ok(())
}

pub(super) async fn stop() -> Result<()> {
    let (_, target) = launchd_identity().await?;
    launchctl(["bootout".into(), target]).await?;
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent stopped");
    Ok(())
}

pub(super) async fn restart() -> Result<()> {
    let paths = ServicePaths::discover()?;
    let (domain, target) = launchd_identity().await?;
    if launchctl_status(["kickstart".into(), "-k".into(), target.clone()])
        .await
        .is_err()
    {
        launchctl([
            "bootstrap".into(),
            domain,
            paths.launch_agent.as_os_str().to_owned(),
        ])
        .await?;
        launchctl(["enable".into(), target.clone()]).await?;
        launchctl(["kickstart".into(), target]).await?;
    }
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent restarted");
    Ok(())
}

pub(super) async fn launchd_identity() -> Result<(OsString, OsString)> {
    let uid = current_uid().await?;
    Ok((
        format!("gui/{uid}").into(),
        format!("gui/{uid}/{SERVICE_LABEL}").into(),
    ))
}

/// The effective user ID, which is also how the privilege gates tell whether
/// they are already running as root.
pub(super) async fn current_uid() -> Result<u32> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .context("cannot determine service user ID")?;
    if !output.status.success() || output.stdout.len() > 16 {
        bail!("cannot determine service user ID");
    }
    std::str::from_utf8(&output.stdout)
        .context("service user ID is not UTF-8")?
        .trim()
        .parse()
        .context("service user ID is invalid")
}

async fn launchctl<const N: usize>(arguments: [OsString; N]) -> Result<()> {
    launchctl_status(arguments)
        .await
        .context("launchctl operation failed")
}

pub(super) async fn launchctl_output<const N: usize>(arguments: [OsString; N]) -> Result<String> {
    let output = Command::new("/bin/launchctl")
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .context("cannot invoke launchctl")?;
    if !output.status.success() {
        bail!("launchctl could not report the service state");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn launchctl_status<const N: usize>(arguments: [OsString; N]) -> Result<()> {
    let status = Command::new("/bin/launchctl")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("cannot invoke launchctl")?;
    if status.success() {
        Ok(())
    } else {
        bail!("launchctl exited unsuccessfully")
    }
}

/// Fails the install when launchd reports the job stopped or exiting non-zero,
/// because the service loads its configuration from the launchd environment.
async fn ensure_service_running(target: &OsString) -> Result<()> {
    let report = launchctl_output(["print".into(), target.clone()]).await?;
    if let Some(code) = last_exit_code(&report)
        && code != 0
    {
        bail!(
            "service started but exited with status {code}; check ~/Library/Logs/flanforged/stderr.log"
        );
    }
    if !report.contains("state = running") {
        bail!("service did not reach the running state; check ~/Library/Logs/flanforged/stderr.log")
    }
    Ok(())
}

pub(super) fn last_exit_code(report: &str) -> Option<i32> {
    report
        .lines()
        .filter_map(|line| line.split_once("last exit code = "))
        .find_map(|(_, value)| value.trim().parse().ok())
}
