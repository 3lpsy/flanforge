use std::path::Path;

use anyhow::{Result, bail};
use flanforge_config::ConfigOverrides;

/// Checks native prerequisites without installing or controlling the service.
pub(super) async fn doctor(path: &Path, overrides: &ConfigOverrides) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let config = flanforge_config::load_config_with_overrides(path, overrides).await?;
        let report = flanforge_libvirt_operator::doctor(&config).await?;
        for check in report.checks() {
            let status = match check.status() {
                flanforge_libvirt_operator::CheckStatus::Passed => "pass",
                flanforge_libvirt_operator::CheckStatus::Failed => "FAIL",
                // Distinct from "pass": this prerequisite was not checked.
                flanforge_libvirt_operator::CheckStatus::Skipped => "n/a",
            };
            println!("{status:<4} {:<24} {}", check.name(), check.detail());
        }
        if !report.is_healthy() {
            bail!("one or more libvirt prerequisites failed")
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        super::privileges::priv_gates(
            path,
            flanforge_cli::DaemonPrivArgs::report_only(),
            overrides,
        )
        .await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (path, overrides);
        bail!("daemon doctor is unsupported on this platform")
    }
}
