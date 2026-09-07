use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use flanforge_config::{ConfigOverrides, load_config_with_overrides};
use flanforge_manager::ConfigHandle;
use flanforge_store::{Event, EventKind, EventSink};
use tokio_util::sync::CancellationToken;

/// Fallback watch period, fixed: a reload trigger is not policy.
const WATCH_INTERVAL: Duration = Duration::from_secs(5);

#[cfg(unix)]
type Hangup = Option<tokio::signal::unix::Signal>;
#[cfg(not(unix))]
type Hangup = ();

/// What a stat of the configuration path proves about its content.
pub(super) type FileStamp = Option<(u64, SystemTime)>;

/// Reloads on SIGHUP and on a periodic stat of the configuration path. The
/// startup stamp is taken by the caller, before the startup load, so an edit
/// written while recovery runs is still picked up.
pub(super) fn spawn_config_reload(
    config_path: PathBuf,
    stamp: FileStamp,
    handle: Arc<ConfigHandle>,
    overrides: ConfigOverrides,
    events: Arc<dyn EventSink>,
    reload_now: Arc<tokio::sync::Notify>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut hangup = hangup_handler();
        let mut stamp = stamp;
        loop {
            let trigger = tokio::select! {
                () = shutdown.cancelled() => return,
                () = wait_for_hangup(&mut hangup) => "sighup",
                // The webui's edit endpoint nudges here after it writes, so
                // the change applies now rather than at the next tick.
                () = reload_now.notified() => "api",
                () = tokio::time::sleep(WATCH_INTERVAL) => {
                    if file_stamp(&config_path).await == stamp {
                        continue;
                    }
                    "file"
                }
            };
            stamp = reload(&config_path, &handle, &overrides, &events, trigger).await;
        }
    });
}

/// Loads, validates, and applies; a failure changes nothing. Returns the stamp
/// taken *before* the read, so a write landing during the reload is seen on the
/// next tick instead of being recorded as already applied.
async fn reload(
    path: &Path,
    handle: &ConfigHandle,
    overrides: &ConfigOverrides,
    events: &Arc<dyn EventSink>,
    trigger: &'static str,
) -> FileStamp {
    let (stamp, loaded) = read_stamped(path, overrides).await;
    let config = match loaded {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(path = %path.display(), trigger, %error, "configuration reload failed; keeping the running configuration");
            return stamp;
        }
    };
    let level = config.logging.level.clone();
    let log_path = config.logging.path.clone();
    match handle.apply(Arc::new(config)) {
        Ok(generation) => {
            tracing::debug!(trigger, generation, "configuration replaced");
            events
                .record(
                    Event::new(EventKind::ConfigReloaded).with_payload(&serde_json::json!({
                        "generation": generation,
                        "trigger": trigger,
                        "restart_pending": handle.restart_pending(),
                    })),
                )
                .await;
            if let Err(error) = flanforge_logging::configure(&level, log_path.as_deref()) {
                tracing::warn!(%error, "cannot apply the reloaded logging settings");
            }
        }
        Err(error) => {
            tracing::error!(path = %path.display(), trigger, %error, "reloaded configuration is invalid; keeping the running configuration");
        }
    }
    stamp
}

/// Stats before reading, so the stamp can only ever describe content at or
/// before what was decoded; a later write compares unequal and reloads again.
pub(super) async fn read_stamped(
    path: &Path,
    overrides: &ConfigOverrides,
) -> (
    FileStamp,
    Result<flanforge_core::Config, flanforge_config::ConfigLoadError>,
) {
    let stamp = file_stamp(path).await;
    (stamp, load_config_with_overrides(path, overrides).await)
}

pub(super) async fn file_stamp(path: &Path) -> FileStamp {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

#[cfg(unix)]
fn hangup_handler() -> Hangup {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        Ok(signal) => Some(signal),
        Err(error) => {
            tracing::error!(%error, "cannot install SIGHUP handler; the file watch still applies");
            None
        }
    }
}

#[cfg(not(unix))]
const fn hangup_handler() -> Hangup {}

#[cfg(unix)]
async fn wait_for_hangup(hangup: &mut Hangup) {
    match hangup {
        Some(signal) => {
            signal.recv().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(not(unix))]
async fn wait_for_hangup(_hangup: &mut Hangup) {
    std::future::pending().await
}

pub(super) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }
            Err(error) => {
                tracing::error!(%error, "cannot install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutdown requested");
}
