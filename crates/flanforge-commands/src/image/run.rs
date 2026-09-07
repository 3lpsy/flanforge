use std::path::Path;

use anyhow::Result;
use flanforge_cli::ImageCommand;
use flanforge_config::ConfigOverrides;

/// Runs a standalone image inspection or immutable import.
///
/// # Errors
/// Returns an error for invalid effective configuration, unsupported hosts,
/// image verification, lock contention, or import failure.
pub async fn run_image_command(
    path: &Path,
    command: ImageCommand,
    overrides: &ConfigOverrides,
) -> Result<()> {
    let config = flanforge_config::load_config_with_overrides(path, overrides).await?;
    #[cfg(target_os = "linux")]
    match command {
        ImageCommand::Inspect(arguments) => {
            let report = flanforge_libvirt_operator::inspect(
                &config,
                &arguments.image,
                arguments.manifest.as_deref(),
            )
            .await?;
            println!("file:          {}", report.file());
            println!("format:        {}", report.format());
            println!("sha256:        {}", report.sha256());
            println!("image bytes:   {}", report.image_bytes());
            println!("virtual bytes: {}", report.virtual_bytes());
            Ok(())
        }
        ImageCommand::Import(arguments) => {
            let cancellation = crate::cancellation::signal_token();
            let report = flanforge_libvirt_operator::import(
                &config,
                &arguments.name,
                &arguments.image,
                arguments.manifest.as_deref(),
                &cancellation,
            )
            .await?;
            println!("published: {}", report.logical_name());
            println!("pool:      {}", report.pool());
            println!("volume:    {}", report.volume_name());
            println!("key:       {}", report.volume_key());
            println!("sha256:    {}", report.sha256());
            Ok(())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, command);
        anyhow::bail!("libvirt image commands are available only on Linux")
    }
}
