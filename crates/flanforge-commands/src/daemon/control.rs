use std::{path::Path, time::Duration};

use flanforge_cli::{DaemonControlArgs, DaemonPrivArgs};
use flanforge_config::ConfigOverrides;

use super::{
    privileges::priv_gates,
    status::{status, wait_until_answering},
};

/// How long the follow-up report waits for a freshly started service to bind.
const SETTLE: Duration = Duration::from_secs(5);

/// Reports what the operator needs after a deploy: whether the service came up,
/// then whether it can actually work. Both macOS gates key on the binary path,
/// so a reinstall silently revokes them and the daemon then fails every
/// allocation while looking healthy (CLI-611).
///
/// Advisory throughout — an unmet gate is printed, never propagated, because
/// an operator may be starting the service before granting it.
pub(super) async fn report_after_control(
    config_path: &Path,
    arguments: DaemonControlArgs,
    overrides: &ConfigOverrides,
) {
    if !arguments.no_status {
        wait_until_answering(config_path, SETTLE, overrides).await;
        if let Err(error) = status(config_path, overrides).await {
            tracing::warn!(%error, "could not report the service status");
        }
    }
    if !arguments.no_privcheck
        && let Err(error) = priv_gates(config_path, DaemonPrivArgs::report_only(), overrides).await
    {
        tracing::warn!(%error, "a host permission gate is unmet; the daemon will fail allocations until it is resolved");
    }
}
