use std::path::Path;

use anyhow::Result;
use flanforge_cli::RuntimeCommand;
use flanforge_config::ConfigOverrides;

/// Runs a configured runtime without contacting Forgejo.
///
/// # Errors
/// Returns an error for invalid effective configuration, unsupported hosts,
/// lock contention, cancellation, guest verification, or exact cleanup.
pub async fn run_runtime_command(
    path: &Path,
    command: RuntimeCommand,
    overrides: &ConfigOverrides,
) -> Result<()> {
    let config = flanforge_config::load_config_with_overrides(path, overrides).await?;
    #[cfg(target_os = "linux")]
    match command {
        RuntimeCommand::Smoke(arguments) => {
            let cancellation = crate::cancellation::signal_token();
            let report =
                flanforge_libvirt_operator::smoke(&config, &arguments.profile, &cancellation)
                    .await?;
            match report.address() {
                Some(address) => println!("verified: {} at {address}", report.vm_name()),
                None => println!("verified: {} over the guest agent", report.vm_name()),
            }
            println!("cleanup:  complete");
            Ok(())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, command);
        anyhow::bail!("libvirt runtime commands are available only on Linux")
    }
}
