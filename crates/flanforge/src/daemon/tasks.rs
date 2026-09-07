use std::{sync::Arc, time::Duration};

use flanforge_core::Config;
use flanforge_manager::{AllocationManager, ConfigHandle};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Spawns the periodic sweep; not spawned when `reap_interval_hours` is zero.
pub(super) fn spawn_reaper(
    manager: AllocationManager,
    handle: Arc<ConfigHandle>,
    shutdown: CancellationToken,
) {
    if handle.current().runtime.reap_interval_hours == 0 {
        tracing::info!("reaper is disabled by configuration");
        return;
    }
    if handle
        .current()
        .runtime
        .tart()
        .is_some_and(|tart| tart.home.is_none())
    {
        tracing::info!(
            "runtime.backend.home is unset; the sweep ages VMs from the library Tart itself resolves"
        );
    }
    tokio::spawn(async move {
        let mut changes = handle.subscribe();
        // The first sweep runs immediately after recovery, which is the moment
        // an orphaned clone from an exhausted cleanup becomes discoverable.
        loop {
            // Re-read before deleting, so a disable written during the sleep
            // stops the sweep instead of costing one more destructive pass.
            if handle.current().runtime.reap_interval_hours == 0 {
                tracing::info!("reaper stopped after a configuration reload disabled it");
                return;
            }
            let started = tokio::time::Instant::now();
            if let Err(error) = manager.sweep(false).await {
                tracing::error!(%error, "reaper sweep failed");
            }
            if !is_sweep_due(&handle, &shutdown, &mut changes, started).await {
                return;
            }
        }
    });
}

/// Waits out the interval measured from `since`, waking on every reload so a
/// shortened or disabled period is not held to the one that was in force.
/// False once the task must stop.
async fn is_sweep_due(
    handle: &ConfigHandle,
    shutdown: &CancellationToken,
    changes: &mut watch::Receiver<Arc<Config>>,
    since: tokio::time::Instant,
) -> bool {
    loop {
        let interval = handle.current().runtime.reap_interval_hours;
        if interval == 0 {
            // The caller reports the disable and stops.
            return true;
        }
        let deadline = since + Duration::from_secs(interval * 3_600);
        tokio::select! {
            () = shutdown.cancelled() => return false,
            result = changes.changed() => {
                if result.is_err() {
                    return false;
                }
            }
            () = tokio::time::sleep_until(deadline) => return true,
        }
    }
}
