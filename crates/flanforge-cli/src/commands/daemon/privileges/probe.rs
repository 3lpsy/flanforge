use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

use flanforge_config::load_config;
use tokio::{net::UdpSocket, time::timeout};

use super::outcome::ProbeOutcome;

/// Bounds a report so the check cannot hang the way the daemon does.
pub(super) const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// The window `--prompt` leaves open for a human to answer macOS.
pub(super) const PROMPT_TIMEOUT: Duration = Duration::from_secs(45);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

const VOLUMES: &str = "/Volumes";
const MDNS_GROUP: &str = "224.0.0.251:5353";
/// A minimal mDNS PTR query for `_services._dns-sd._udp.local`; sending it is
/// the local-network access macOS gates.
const MDNS_QUERY: &[u8] =
    b"\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x09_services\x07_dns-sd\x04_udp\x05local\x00\x00\x0c\x00\x01";

#[derive(Clone, Debug)]
pub(super) struct Probes {
    pub(super) volume: ProbeOutcome,
    pub(super) network: ProbeOutcome,
}

/// Both gates, probed in this process. The consent prompt macOS raises is
/// attributed to this executable.
pub(super) async fn run_probes(config_path: &Path, bound: Duration) -> Probes {
    Probes {
        volume: match tart_home(config_path).await {
            Ok(Some(directory)) => probe_volume(&directory, bound).await,
            Ok(None) => ProbeOutcome::Skipped(
                "runtime.tart_home is not on a volume under /Volumes".to_owned(),
            ),
            Err(detail) => ProbeOutcome::Failed(detail),
        },
        network: probe_local_network(bound).await,
    }
}

/// Reads the configured library exactly as the daemon's first Tart call does.
pub(super) async fn probe_volume(directory: &Path, bound: Duration) -> ProbeOutcome {
    let listing = async {
        let mut entries = tokio::fs::read_dir(directory).await?;
        entries.next_entry().await.map(|_| ())
    };
    ProbeOutcome::classify(timeout(bound, listing).await.ok())
}

/// Sends on the local network, then retries within the bound: the prompt is
/// answered out of band, so a grant only shows up on a later attempt.
pub(super) async fn probe_local_network(bound: Duration) -> ProbeOutcome {
    let deadline = tokio::time::Instant::now() + bound;
    let attempt = ATTEMPT_TIMEOUT.min(bound);
    let mut outcome = send_local(attempt).await;
    while !outcome.is_allowed() && tokio::time::Instant::now() + RETRY_INTERVAL < deadline {
        tokio::time::sleep(RETRY_INTERVAL).await;
        outcome = send_local(attempt).await;
    }
    outcome
}

async fn send_local(bound: Duration) -> ProbeOutcome {
    let send = async {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.send_to(MDNS_QUERY, MDNS_GROUP).await.map(|_| ())
    };
    ProbeOutcome::classify(timeout(bound, send).await.ok())
}

async fn tart_home(config_path: &Path) -> Result<Option<PathBuf>, String> {
    let config = load_config(config_path)
        .await
        .map_err(|error| format!("cannot load {}: {error}", config_path.display()))?;
    Ok(config
        .runtime
        .tart_home
        .as_deref()
        .filter(|home| removable_volume(home).is_some())
        .map(Path::to_path_buf))
}

/// The volume root of a path under `/Volumes`, and nothing otherwise.
pub(super) fn removable_volume(path: &Path) -> Option<PathBuf> {
    let mut components = path.strip_prefix(VOLUMES).ok()?.components();
    let Some(Component::Normal(name)) = components.next() else {
        return None;
    };
    Some(Path::new(VOLUMES).join(name))
}
