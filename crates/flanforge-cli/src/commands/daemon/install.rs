use std::path::Path;

use anyhow::{Context, Result};
use flanforge_config::load_config;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::paths::{SERVICE_LABEL, ServicePaths};

pub(super) async fn install(config_path: &Path) -> Result<()> {
    let config_path = tokio::fs::canonicalize(config_path)
        .await
        .with_context(|| format!("cannot resolve configuration {}", config_path.display()))?;
    let config = load_config(&config_path)
        .await
        .context("cannot load service configuration")?;
    config
        .ensure_valid()
        .context("invalid service configuration")?;

    let paths = ServicePaths::discover()?;
    tokio::fs::create_dir_all(&paths.binary_dir)
        .await
        .context("cannot create service binary directory")?;
    tokio::fs::create_dir_all(&paths.log_dir)
        .await
        .context("cannot create service log directory")?;
    tokio::fs::create_dir_all(
        paths
            .launch_agent
            .parent()
            .ok_or_else(|| anyhow::anyhow!("LaunchAgents path has no parent"))?,
    )
    .await
    .context("cannot create LaunchAgents directory")?;

    install_current_binary(&paths.binary).await?;
    let plist = launch_agent_plist(&paths, &config_path);
    atomic_write(&paths.launch_agent, plist.as_bytes(), 0o600).await?;

    // Installing only writes files. Starting is a separate step so an install
    // cannot collide with a daemon that is already holding the state lock.
    tracing::info!(
        path = %paths.launch_agent.display(),
        config = %config_path.display(),
        "LaunchAgent written"
    );
    tracing::info!(
        service = SERVICE_LABEL,
        "macOS LaunchAgent installed; start it with `flanforged daemon start`"
    );
    Ok(())
}

async fn install_current_binary(destination: &Path) -> Result<()> {
    let source = std::env::current_exe().context("cannot locate current executable")?;
    let bytes = tokio::fs::read(&source)
        .await
        .context("cannot read current executable")?;
    atomic_write(destination, &bytes, 0o700)
        .await
        .context("cannot install service binary")?;
    // The firewall identifies an unsigned binary by path, so the operator needs
    // to see which path now holds the service.
    tracing::info!(
        source = %source.display(),
        destination = %destination.display(),
        bytes = bytes.len(),
        "service binary copied"
    );
    Ok(())
}

async fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("destination has no parent"))?;
    let temporary = parent.join(format!(".flanforged-{}.tmp", Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(&temporary)
        .await
        .context("cannot create temporary installation file")?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("cannot write installation file");
    }
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("cannot atomically install file");
    }
    Ok(())
}

pub(super) fn launch_agent_plist(paths: &ServicePaths, config_path: &Path) -> String {
    let binary = xml_escape(&paths.binary.to_string_lossy());
    let config = xml_escape(&config_path.to_string_lossy());
    let working = xml_escape(&paths.binary_dir.to_string_lossy());
    let stdout = xml_escape(&paths.stdout_log.to_string_lossy());
    let stderr = xml_escape(&paths.stderr_log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>--config</string>
    <string>{config}</string>
    <string>daemon</string>
    <string>run</string>
  </array>
  <key>WorkingDirectory</key><string>{working}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
