use anyhow::Result;
use clap::Parser;
use flanforge_cli::{
    Arguments, DaemonCommand, TopCommand, run_allocation_command, run_config_command,
    run_daemon_command, run_profile_command, run_reaper_command, selected_config_path,
};

mod daemon;

#[tokio::main]
async fn main() -> Result<()> {
    flanforge_logging::init();
    match run().await {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::error!(error = ?error, "flanforged stopped with an error");
            Err(error)
        }
    }
}

async fn run() -> Result<()> {
    let arguments = Arguments::parse();
    let config_path = selected_config_path(arguments.config)?;
    match arguments.command {
        TopCommand::Daemon(arguments) => match arguments.command {
            DaemonCommand::Run => daemon::run_daemon(&config_path).await,
            command => run_daemon_command(&config_path, command).await,
        },
        TopCommand::Config(arguments) => run_config_command(&config_path, arguments.command).await,
        TopCommand::Profile(arguments) => {
            run_profile_command(&config_path, arguments.command).await
        }
        TopCommand::Allocation(arguments) => {
            run_allocation_command(&config_path, arguments.command).await
        }
        TopCommand::Reaper(arguments) => run_reaper_command(&config_path, arguments.command).await,
    }
}
