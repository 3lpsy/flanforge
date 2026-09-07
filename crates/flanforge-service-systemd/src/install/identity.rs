use std::{io, os::unix::fs::PermissionsExt, path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use tokio::{process::Command, time::timeout};

use crate::{
    constants::{
        CHOWN_PATH, COMMAND_TIMEOUT, DEFAULT_USER, GETENT_NOT_FOUND_EXIT_CODE, GETENT_PATH,
        ID_PATH, NLOGIN_PATH, USERADD_PATH,
    },
    paths::SystemdPaths,
    user::{PathAccess, ensure_user_name, is_user_path_accessible},
};

pub(crate) async fn ensure_root() -> Result<()> {
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
    .context("cannot determine effective user")?;
    if !output.status.success() || output.stdout.len() > 16 {
        bail!("cannot determine effective user");
    }
    if output.stdout != b"0\n" && output.stdout != b"0" {
        bail!("systemd installation requires root; rerun with sudo");
    }
    Ok(())
}

pub(crate) async fn ensure_service_user(paths: &SystemdPaths) -> Result<()> {
    let status = bounded_status(
        Command::new(GETENT_PATH)
            .args(["passwd", DEFAULT_USER])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "cannot inspect service account",
    )
    .await?;
    if status.success() {
        return Ok(());
    }
    if !is_getent_user_missing(status.code()) {
        bail!("cannot determine whether the {DEFAULT_USER} service account exists");
    }
    let status = bounded_status(
        Command::new(USERADD_PATH)
            .args(["--system", "--user-group", "--home-dir"])
            .arg(&paths.state_dir)
            .args(["--shell", NLOGIN_PATH, DEFAULT_USER])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "cannot create service account",
    )
    .await?;
    if status.success() {
        Ok(())
    } else {
        bail!("cannot create the {DEFAULT_USER} service account")
    }
}

pub(crate) async fn make_configuration_readable(config_path: &Path) -> Result<()> {
    let status = bounded_status(
        Command::new(CHOWN_PATH)
            .arg(format!("root:{DEFAULT_USER}"))
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "cannot set configuration ownership",
    )
    .await?;
    if !status.success() {
        bail!("cannot make the configuration readable by {DEFAULT_USER}");
    }
    tokio::fs::set_permissions(config_path, std::fs::Permissions::from_mode(0o640))
        .await
        .context("cannot set configuration permissions")
}

pub(crate) async fn ensure_state_access(user: &str, state_dir: &Path) -> Result<()> {
    ensure_user_name(user)?;
    let existed = match tokio::fs::metadata(state_dir).await {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => bail!(
            "configured state path {} is not a directory",
            state_dir.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot inspect configured state directory {}",
                    state_dir.display()
                )
            });
        }
    };
    if existed {
        return Ok(());
    }
    tokio::fs::create_dir_all(state_dir)
        .await
        .context("cannot create configured state directory")?;
    let status = bounded_status(
        Command::new(CHOWN_PATH)
            .args(["--", owner_spec(user)])
            .arg(state_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "cannot set new state directory ownership",
    )
    .await?;
    if status.success() {
        Ok(())
    } else {
        bail!("cannot make the new state directory writable by {user}")
    }
}

pub(crate) async fn ensure_identity_access(
    user: &str,
    config_path: &Path,
    state_dir: &Path,
) -> Result<()> {
    ensure_user_name(user)?;
    ensure_user_test(user, "-r", config_path, "read the configuration").await?;
    ensure_user_test(user, "-w", state_dir, "write the state directory").await
}

/// The database directory must be writable before the first start, not at it:
/// this is the highest-probability field failure of the `db_path` default.
pub(crate) async fn ensure_db_access(user: &str, db_dir: &Path) -> Result<()> {
    ensure_state_access(user, db_dir).await?;
    ensure_user_test(
        user,
        "-w",
        db_dir,
        "write the database directory; grant access or point db.db_path elsewhere",
    )
    .await
}

async fn ensure_user_test(user: &str, mode: &str, path: &Path, operation: &str) -> Result<()> {
    let access = match mode {
        "-r" => PathAccess::Read,
        "-w" => PathAccess::Write,
        "-x" => PathAccess::Search,
        _ => bail!("unsupported service identity access mode"),
    };
    if is_user_path_accessible(user, path, access).await? {
        Ok(())
    } else {
        bail!(
            "installed unit user {user} cannot {operation} at {}",
            path.display()
        )
    }
}

async fn bounded_status(
    command: &mut Command,
    context: &'static str,
) -> Result<std::process::ExitStatus> {
    timeout(COMMAND_TIMEOUT, command.kill_on_drop(true).status())
        .await
        .context("service identity command timed out")?
        .context(context)
}

pub(crate) const fn is_getent_user_missing(code: Option<i32>) -> bool {
    matches!(code, Some(GETENT_NOT_FOUND_EXIT_CODE))
}

pub(crate) fn owner_spec(user: &str) -> &str {
    user
}
