use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use flanforge_service::{LogOptions, ServicePlatform, ServiceStatus};
use tokio::{process::Command, time::timeout};

use crate::{
    constants::{
        COMMAND_TIMEOUT, CONTROL_TIMEOUT, JOURNALCTL_PATH, MAX_STATUS_BYTES, SERVICE_NAME,
        SYSTEMCTL_PATH,
    },
    paths::{SystemdPaths, is_present},
};

pub(crate) async fn start() -> Result<()> {
    systemctl(&["start", SERVICE_NAME], CONTROL_TIMEOUT).await?;
    ensure_active().await?;
    tracing::info!(service = SERVICE_NAME, "systemd service started");
    Ok(())
}

pub(crate) async fn stop() -> Result<()> {
    systemctl(&["stop", SERVICE_NAME], CONTROL_TIMEOUT).await?;
    tracing::info!(service = SERVICE_NAME, "systemd service stopped");
    Ok(())
}

pub(crate) async fn restart() -> Result<()> {
    systemctl(&["restart", SERVICE_NAME], CONTROL_TIMEOUT).await?;
    ensure_active().await?;
    tracing::info!(service = SERVICE_NAME, "systemd service restarted");
    Ok(())
}

pub(crate) async fn status() -> Result<ServiceStatus> {
    let paths = SystemdPaths::default();
    if !is_present(&paths.definition).await? {
        return Ok(service_status(
            paths,
            false,
            "not installed",
            None,
            None,
            None,
        ));
    }
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(SYSTEMCTL_PATH)
            .args([
                "show",
                SERVICE_NAME,
                "--property=LoadState,ActiveState,SubState,MainPID,ExecMainStatus,User",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("systemctl status timed out")?
    .context("cannot inspect systemd service")?;
    if !output.status.success() || output.stdout.len() > MAX_STATUS_BYTES {
        bail!("systemd could not report the service state");
    }
    let report = String::from_utf8(output.stdout).context("systemd status is not UTF-8")?;
    let active = property(&report, "ActiveState").unwrap_or("unknown");
    let sub = property(&report, "SubState").unwrap_or("unknown");
    let user = property(&report, "User")
        .filter(|value| !value.is_empty())
        .unwrap_or("root")
        .to_owned();
    let state = format!("{active} ({sub})");
    let pid = property(&report, "MainPID")
        .and_then(|value| value.parse().ok())
        .filter(|pid| *pid != 0);
    let last_exit = property(&report, "ExecMainStatus").and_then(|value| value.parse().ok());
    Ok(service_status(
        paths,
        true,
        &state,
        pid,
        last_exit,
        Some(user),
    ))
}

pub(crate) async fn effective_user() -> Result<String> {
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(SYSTEMCTL_PATH)
            .args(["show", SERVICE_NAME, "--property=User"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("systemctl user query timed out")?
    .context("cannot inspect the installed systemd user")?;
    if !output.status.success() || output.stdout.len() > 4_096 {
        bail!("systemd could not report the installed service user");
    }
    let report = String::from_utf8(output.stdout).context("systemd user report is not UTF-8")?;
    let user = property(&report, "User")
        .filter(|value| !value.is_empty())
        .unwrap_or("root");
    crate::user::ensure_user_name(user)?;
    Ok(user.to_owned())
}

pub(crate) async fn logs(options: LogOptions) -> Result<()> {
    if options.is_stderr() {
        tracing::warn!(
            "systemd journals stdout and stderr together; --stderr reads the same journal"
        );
    }
    let mut command = journalctl_command(options);
    let status = if let Some(bound) = journalctl_timeout(options) {
        timeout(bound, command.status())
            .await
            .context("journalctl timed out")?
    } else {
        command.status().await
    }
    .context("cannot invoke journalctl")?;
    if status.success() {
        Ok(())
    } else {
        bail!("journalctl exited with {status}")
    }
}

pub(crate) fn journalctl_command(options: LogOptions) -> Command {
    let mut command = Command::new(JOURNALCTL_PATH);
    command
        .args(["--unit", SERVICE_NAME, "--lines"])
        .arg(options.lines().to_string())
        .kill_on_drop(true);
    if options.is_following() {
        command.arg("--follow");
    }
    command
}

pub(crate) const fn journalctl_timeout(options: LogOptions) -> Option<Duration> {
    if options.is_following() {
        None
    } else {
        Some(COMMAND_TIMEOUT)
    }
}

async fn ensure_active() -> Result<()> {
    let status = timeout(
        COMMAND_TIMEOUT,
        Command::new(SYSTEMCTL_PATH)
            .args(["is-active", "--quiet", SERVICE_NAME])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("systemctl active check timed out")?
    .context("cannot inspect systemd service")?;
    if status.success() {
        Ok(())
    } else {
        bail!("service did not reach the active state; inspect `flanforged daemon logs`")
    }
}

pub(crate) async fn systemctl(arguments: &[&str], bound: Duration) -> Result<()> {
    let status = timeout(
        bound,
        Command::new(SYSTEMCTL_PATH)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("systemctl operation timed out")?
    .context("cannot invoke systemctl")?;
    if status.success() {
        Ok(())
    } else {
        bail!("systemctl operation failed")
    }
}

fn service_status(
    paths: SystemdPaths,
    is_installed: bool,
    state: &str,
    pid: Option<u32>,
    last_exit: Option<i32>,
    user: Option<String>,
) -> ServiceStatus {
    ServiceStatus {
        platform: ServicePlatform::Systemd,
        label: SERVICE_NAME,
        binary: paths.binary,
        definition: paths.definition,
        is_installed,
        state: state.to_owned(),
        pid,
        last_exit,
        user,
    }
}

pub(crate) fn property<'a>(report: &'a str, name: &str) -> Option<&'a str> {
    report.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key == name).then_some(value.trim())
    })
}
