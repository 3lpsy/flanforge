use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use flanforge_cli::{Arguments, DaemonCommand, TopCommand};
use flanforge_commands::{
    run_allocation_command, run_config_command, run_daemon_command, run_hot_command,
    run_image_command, run_profile_command, run_reaper_command, run_runtime_command,
    run_webui_command, selected_config_path,
};
use flanforge_config::ConfigOverrides;

mod daemon;

#[tokio::main]
async fn main() -> Result<ExitCode> {
    flanforge_logging::init();
    match run().await {
        Ok(status) => Ok(ExitCode::from(status)),
        Err(error) => {
            tracing::error!(error = ?error, "flanforged stopped with an error");
            Err(error)
        }
    }
}

/// Every command but `daemon run` reports success as 0; see `daemon::run` for
/// the statuses a stop can produce.
async fn run() -> Result<u8> {
    #[cfg(target_os = "linux")]
    if is_libvirt_helper_invocation()? {
        return flanforge_runtime_libvirt::run_helper()
            .map(|()| daemon::EXIT_OK)
            .map_err(Into::into);
    }
    let arguments = Arguments::parse();
    let config_path = selected_config_path(arguments.config)?;
    let overrides = ConfigOverrides::new(
        arguments
            .overrides
            .iter()
            .map(|entry| (entry.key(), entry.value())),
    );
    if let TopCommand::Daemon(arguments) = &arguments.command
        && matches!(arguments.command, DaemonCommand::Run)
    {
        return daemon::run_daemon(&config_path, &overrides).await;
    }
    run_command(arguments.command, &config_path, &overrides)
        .await
        .map(|()| daemon::EXIT_OK)
}

async fn run_command(
    command: TopCommand,
    config_path: &std::path::Path,
    overrides: &ConfigOverrides,
) -> Result<()> {
    match command {
        TopCommand::Daemon(arguments) => {
            run_daemon_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Config(arguments) => {
            ensure_no_overrides(overrides, "config")?;
            run_config_command(config_path, arguments.command).await
        }
        TopCommand::Profile(arguments) => {
            ensure_no_overrides(overrides, "profile")?;
            run_profile_command(config_path, arguments.command).await
        }
        TopCommand::Allocation(arguments) => {
            run_allocation_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Reaper(arguments) => {
            run_reaper_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Hot(arguments) => {
            run_hot_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Image(arguments) => {
            run_image_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Runtime(arguments) => {
            run_runtime_command(config_path, arguments.command, overrides).await
        }
        TopCommand::Webui(arguments) => {
            run_webui_command(config_path, arguments.command, overrides).await
        }
    }
}

#[cfg(target_os = "linux")]
fn is_libvirt_helper_invocation() -> Result<bool> {
    use std::ffi::OsStr;

    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref()
        != Some(OsStr::new(
            flanforge_runtime_libvirt::LIBVIRT_HELPER_ARGUMENT,
        ))
    {
        return Ok(false);
    }
    anyhow::ensure!(
        arguments.next().is_none(),
        "libvirt helper accepts no command-line values"
    );
    Ok(true)
}

fn ensure_no_overrides(overrides: &ConfigOverrides, command: &str) -> Result<()> {
    anyhow::ensure!(
        overrides.is_empty(),
        "{command} edits the TOML document and cannot combine with transient --set values"
    );
    Ok(())
}
