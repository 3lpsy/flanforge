use std::path::Path;

use anyhow::{Result, bail};
use flanforge_config::ConfigOverrides;

use flanforge_cli::DaemonCommand;

use super::{control, doctor::doctor, privileges::priv_gates, service, status::status};

/// Installs or controls the native launchd or systemd service.
///
/// # Errors
///
/// Returns an error for invalid service configuration or provider failures.
pub async fn run_daemon_command(
    path: &Path,
    command: DaemonCommand,
    overrides: &ConfigOverrides,
) -> Result<()> {
    let service = service::native();
    match command {
        DaemonCommand::Install => {
            ensure_not_transient(overrides, "install")?;
            flanforge_config::ensure_no_ambient_service_overrides()?;
            service.install(path).await?;
            Ok(())
        }
        DaemonCommand::Start(arguments) => {
            ensure_not_transient(overrides, "start")?;
            service.start().await?;
            control::report_after_control(path, arguments, overrides).await;
            Ok(())
        }
        DaemonCommand::Stop => {
            ensure_not_transient(overrides, "stop")?;
            service.stop().await?;
            Ok(())
        }
        DaemonCommand::Restart(arguments) => {
            ensure_not_transient(overrides, "restart")?;
            service.restart().await?;
            control::report_after_control(path, arguments, overrides).await;
            Ok(())
        }
        DaemonCommand::Status => status(path, overrides).await,
        DaemonCommand::Doctor => doctor(path, overrides).await,
        DaemonCommand::Logs(arguments) => {
            ensure_not_transient(overrides, "logs")?;
            service
                .logs(path, arguments.follow, arguments.lines, arguments.stderr)
                .await?;
            Ok(())
        }
        DaemonCommand::Priv(arguments) => priv_gates(path, arguments, overrides).await,
        DaemonCommand::Run => bail!("daemon run must execute in the foreground"),
    }
}

fn ensure_not_transient(overrides: &ConfigOverrides, command: &str) -> Result<()> {
    if !overrides.is_empty() {
        bail!(
            "daemon {command} cannot persist --set values; put them in TOML or the native service environment"
        );
    }
    Ok(())
}
