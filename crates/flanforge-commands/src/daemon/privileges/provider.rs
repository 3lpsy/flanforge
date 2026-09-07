use std::path::Path;

use anyhow::Result;
use flanforge_config::ConfigOverrides;

use flanforge_cli::DaemonPrivArgs;

pub(crate) async fn priv_gates(
    path: &Path,
    arguments: DaemonPrivArgs,
    overrides: &ConfigOverrides,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        super::run::priv_gates(path, arguments, overrides).await
    }
    #[cfg(target_os = "linux")]
    {
        super::linux::priv_gates(path, arguments, overrides).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (path, arguments, overrides);
        anyhow::bail!("daemon privileges are supported only on macOS and Linux")
    }
}
