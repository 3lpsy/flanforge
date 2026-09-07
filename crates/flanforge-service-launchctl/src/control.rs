use std::{ffi::OsString, process::Stdio};

use anyhow::{Context, Result, bail};
use flanforge_service::{ServicePlatform, ServiceStatus};
use tokio::{process::Command, time::timeout};

use crate::{
    constants::{
        COMMAND_TIMEOUT, ID_PATH, LAUNCHCTL_PATH, MAX_STATUS_BYTES, NOT_LOADED_EXIT_CODE,
        SERVICE_LABEL,
    },
    paths::{discover, is_present},
};

pub(crate) async fn start() -> Result<()> {
    let paths = discover()?;
    if !is_present(&paths.definition).await? {
        bail!("LaunchAgent is not installed; run `flanforged daemon install`");
    }
    let (domain, target) = launchd_identity().await?;
    // Sampled before the start: launchd reports the *previous* run's exit code
    // under KeepAlive, so an unchanged value says nothing about this one.
    let previous_exit = previous_exit_code(&target).await;
    if !launchctl_success(&[
        "bootstrap".into(),
        domain,
        paths.definition.as_os_str().to_owned(),
    ])
    .await?
    {
        tracing::debug!(
            service = SERVICE_LABEL,
            "LaunchAgent was already loaded or could not be bootstrapped; trying kickstart"
        );
    }
    require_launchctl(&["enable".into(), target.clone()]).await?;
    require_launchctl(&["kickstart".into(), target.clone()]).await?;
    ensure_service_running(&target, previous_exit).await?;
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent started");
    Ok(())
}

pub(crate) async fn stop() -> Result<()> {
    let (_, target) = launchd_identity().await?;
    require_launchctl(&["bootout".into(), target]).await?;
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent stopped");
    Ok(())
}

pub(crate) async fn restart() -> Result<()> {
    let paths = discover()?;
    let (domain, target) = launchd_identity().await?;
    if !launchctl_success(&["kickstart".into(), "-k".into(), target.clone()]).await? {
        require_launchctl(&[
            "bootstrap".into(),
            domain,
            paths.definition.as_os_str().to_owned(),
        ])
        .await?;
        require_launchctl(&["enable".into(), target.clone()]).await?;
        require_launchctl(&["kickstart".into(), target]).await?;
    }
    tracing::info!(service = SERVICE_LABEL, "macOS LaunchAgent restarted");
    Ok(())
}

pub(crate) async fn status() -> Result<ServiceStatus> {
    let paths = discover()?;
    if !is_present(&paths.definition).await? {
        return Ok(service_status(paths, false, "not installed", None, None));
    }
    let (_, target) = launchd_identity().await?;
    let output = launchctl_output(&["print".into(), target]).await?;
    if !output.status.success() {
        if is_not_loaded_exit(output.status.code()) {
            return Ok(service_status(paths, true, "not loaded", None, None));
        }
        bail!("launchctl could not report the service state");
    }
    let report = String::from_utf8(output.stdout).context("launchctl status is not UTF-8")?;
    let state = report_field(&report, "state = ").unwrap_or("unknown");
    let pid = report_field(&report, "pid = ").and_then(|value| value.parse().ok());
    Ok(service_status(
        paths,
        true,
        state,
        pid,
        last_exit_code(&report),
    ))
}

/// Returns the numeric UID of the current launchctl service user.
///
/// # Errors
/// Returns an error when `id` fails, times out, or reports malformed output.
pub async fn current_uid() -> Result<u32> {
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(ID_PATH)
            .arg("-u")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("id timed out")?
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

fn service_status(
    paths: flanforge_paths::LaunchdPaths,
    is_installed: bool,
    state: &str,
    pid: Option<u32>,
    last_exit: Option<i32>,
) -> ServiceStatus {
    ServiceStatus {
        platform: ServicePlatform::Launchd,
        label: SERVICE_LABEL,
        binary: paths.binary,
        definition: paths.definition,
        is_installed,
        state: state.to_owned(),
        pid,
        last_exit,
        user: None,
    }
}

async fn launchd_identity() -> Result<(OsString, OsString)> {
    let uid = current_uid().await?;
    Ok((
        format!("gui/{uid}").into(),
        format!("gui/{uid}/{SERVICE_LABEL}").into(),
    ))
}

async fn require_launchctl(arguments: &[OsString]) -> Result<()> {
    if launchctl_success(arguments).await? {
        Ok(())
    } else {
        bail!("launchctl exited unsuccessfully")
    }
}

async fn launchctl_success(arguments: &[OsString]) -> Result<bool> {
    let status = timeout(
        COMMAND_TIMEOUT,
        Command::new(LAUNCHCTL_PATH)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("launchctl timed out")?
    .context("cannot invoke launchctl")?;
    Ok(status.success())
}

async fn launchctl_output(arguments: &[OsString]) -> Result<std::process::Output> {
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(LAUNCHCTL_PATH)
            .args(arguments)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("launchctl timed out")?
    .context("cannot invoke launchctl")?;
    if output.stdout.len() > MAX_STATUS_BYTES {
        bail!("launchctl status exceeds the supported size");
    }
    Ok(output)
}

/// The job's last exit code before a start, so a value left by an earlier run
/// is not read as this one's. Absent when the job is not loaded at all.
async fn previous_exit_code(target: &OsString) -> Option<i32> {
    let output = launchctl_output(&["print".into(), target.clone()])
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    last_exit_code(&String::from_utf8(output.stdout).ok()?)
}

async fn ensure_service_running(target: &OsString, previous_exit: Option<i32>) -> Result<()> {
    let output = launchctl_output(&["print".into(), target.clone()]).await?;
    if !output.status.success() {
        bail!("launchctl could not report the started service");
    }
    let report = String::from_utf8(output.stdout).context("launchctl status is not UTF-8")?;
    ensure_started(&report, previous_exit)
}

/// Split from the launchctl call so the stale-code rule is testable.
pub(crate) fn ensure_started(report: &str, previous_exit: Option<i32>) -> Result<()> {
    let exit = last_exit_code(report).filter(|code| *code != 0);
    if !report.contains("state = running") {
        if let Some(code) = exit {
            bail!(
                "service started but exited with status {code}; inspect `flanforged daemon logs`"
            );
        }
        bail!("service did not reach the running state; inspect `flanforged daemon logs`");
    }
    // Running. A code that changed across the start means it died and launchd
    // brought it back; an unchanged one describes a run that already ended --
    // under KeepAlive that is every ordinary restart, including a stop during a
    // job, which exits non-zero by design.
    if let Some(code) = exit
        && exit != previous_exit
    {
        bail!(
            "service restarted after exiting with status {code}; inspect `flanforged daemon logs`"
        );
    }
    Ok(())
}

pub(crate) fn is_not_loaded_exit(code: Option<i32>) -> bool {
    code == Some(NOT_LOADED_EXIT_CODE)
}

pub(crate) fn last_exit_code(report: &str) -> Option<i32> {
    report
        .lines()
        .filter_map(|line| line.split_once("last exit code = "))
        .find_map(|(_, value)| value.trim().parse().ok())
}

fn report_field<'a>(report: &'a str, key: &str) -> Option<&'a str> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(key))
        .map(str::trim)
}
