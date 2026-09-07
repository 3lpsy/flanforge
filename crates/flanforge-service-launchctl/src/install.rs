use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_core::{Config, RuntimeBackendKind};
use flanforge_service_files::{atomic_write, install_current_binary};

use crate::{
    constants::SERVICE_LABEL,
    paths::{LaunchdPaths, discover},
};

pub(crate) async fn install(config: &Config, config_path: &Path) -> Result<()> {
    if config.runtime.backend_kind() != RuntimeBackendKind::Tart {
        bail!("macOS launchctl installation requires the Tart runtime backend");
    }
    let paths = discover()?;
    tokio::fs::create_dir_all(&paths.binary_dir)
        .await
        .context("cannot create service binary directory")?;
    tokio::fs::create_dir_all(&paths.log_dir)
        .await
        .context("cannot create service log directory")?;
    tokio::fs::create_dir_all(
        paths
            .definition
            .parent()
            .ok_or_else(|| anyhow::anyhow!("LaunchAgents path has no parent"))?,
    )
    .await
    .context("cannot create LaunchAgents directory")?;

    install_current_binary(&paths.binary, 0o700).await?;
    let plist = launch_agent_plist(&paths, config_path, config.server.shutdown_grace_seconds)?;
    atomic_write(&paths.definition, plist.as_bytes(), 0o600).await?;
    tracing::info!(path = %paths.definition.display(), config = %config_path.display(), "LaunchAgent written");
    tracing::info!(
        service = SERVICE_LABEL,
        "macOS LaunchAgent installed; start it with `flanforged daemon start`"
    );
    Ok(())
}

pub(crate) fn launch_agent_plist(
    paths: &LaunchdPaths,
    config_path: &Path,
    shutdown_grace_seconds: u64,
) -> Result<String> {
    let binary = xml_escape(path_text(&paths.binary)?);
    let config = xml_escape(path_text(config_path)?);
    let working = xml_escape(path_text(&paths.binary_dir)?);
    let stdout = xml_escape(path_text(&paths.stdout_log)?);
    let stderr = xml_escape(path_text(&paths.stderr_log)?);
    let exit_timeout = shutdown_grace_seconds.saturating_add(5);
    Ok(format!(
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
  <key>ExitTimeOut</key><integer>{exit_timeout}</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict>
</plist>
"#
    ))
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("launchctl path is not UTF-8: {}", path.display()))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
