use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use tokio::{process::Command, time::timeout};

use crate::constants::{COMMAND_TIMEOUT, RUNUSER_PATH, TEST_PATH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathAccess {
    Read,
    Write,
    Search,
}

impl PathAccess {
    const fn flag(self) -> &'static str {
        match self {
            Self::Read => "-r",
            Self::Write => "-w",
            Self::Search => "-x",
        }
    }
}

/// Tests one filesystem permission as the installed service user.
///
/// # Errors
/// Returns an error when the user is invalid or the bounded probe cannot run.
pub async fn is_user_path_accessible(user: &str, path: &Path, access: PathAccess) -> Result<bool> {
    ensure_user_name(user)?;
    flanforge_paths::ensure_absolute_normalized(path)
        .context("service-user probe path must be absolute and normalized")?;
    let status = timeout(
        COMMAND_TIMEOUT,
        Command::new(RUNUSER_PATH)
            .args(["--user", user, "--", TEST_PATH, access.flag()])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("service-user path probe timed out")?
    .context("cannot run service-user path probe")?;
    Ok(status.success())
}

/// Runs a fixed executable and argument vector as the installed service user.
///
/// # Errors
/// Returns an error when inputs are invalid or the bounded command cannot run.
pub async fn run_as_user(
    user: &str,
    program: &Path,
    arguments: &[&str],
) -> Result<std::process::ExitStatus> {
    ensure_user_name(user)?;
    flanforge_paths::ensure_absolute_normalized(program)
        .context("service-user program must be absolute and normalized")?;
    if arguments.len() > 32 || arguments.iter().any(|argument| argument.len() > 4_096) {
        bail!("service-user argument vector exceeds the supported bound");
    }
    timeout(
        COMMAND_TIMEOUT,
        Command::new(RUNUSER_PATH)
            .args(["--user", user, "--"])
            .arg(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("service-user command timed out")?
    .context("cannot run service-user command")
}

pub(crate) fn ensure_user_name(user: &str) -> Result<()> {
    let mut bytes = user.bytes();
    let Some(first) = bytes.next() else {
        bail!("installed systemd unit has an empty user");
    };
    if user.len() > 256
        || !(first.is_ascii_alphanumeric() || first == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        bail!("installed systemd unit has an invalid user");
    }
    Ok(())
}
