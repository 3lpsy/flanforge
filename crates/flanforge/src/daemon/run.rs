use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use flanforge_auth::OidcVerifier;
use flanforge_config::load_config;
use flanforge_core::Config;
use flanforge_forgejo::{ForgejoClient, read_secret_file};
use flanforge_manager::{AllocationManager, ConfigHandle, ensure_operator_token};
use flanforge_router::{BoundedListener, build_operator_router, build_router, with_peer_info};
use flanforge_routes::{AppState, OperatorState};
use flanforge_runtime::{FlanForgeWorker, ensure_guest_known_hosts};
use flanforge_store::{JsonStateStore, JsonWarmImageStore};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::{
    signals::{file_stamp, shutdown_signal, spawn_config_reload},
    tasks::spawn_reaper,
};

/// Ingress bounds: far above the one allocation the daemon serves, far below
/// the descriptor limit, and short enough that a stalled peer cannot linger.
const MAX_CONNECTIONS: usize = 128;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn run_daemon(config_path: &Path) -> Result<()> {
    tracing::debug!(config_path = %config_path.display(), "loading service configuration");
    // Stamped before the read: an edit written while recovery runs then differs
    // from it and reaches the watcher instead of being skipped as applied.
    let stamp = file_stamp(config_path).await;
    let config = Arc::new(load_config(config_path).await?);
    config
        .ensure_valid()
        .context("invalid service configuration")?;
    flanforge_logging::configure(&config.logging.level, config.logging.path.as_deref())
        .map_err(anyhow::Error::msg)
        .context("cannot configure logging")?;
    tracing::info!(
        profiles = config.profiles.len(),
        listen = %config.server.listen,
        "service configuration loaded"
    );

    ensure_guest_known_hosts(&config.guest)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .context("guest host-key anchor is unusable")?;

    let handle = ConfigHandle::new(Arc::clone(&config));
    let manager = open_manager(&config, Arc::clone(&handle)).await?;
    manager
        .recover()
        .await
        .context("cannot reconcile interrupted allocations")?;

    let verifier = Arc::new(
        OidcVerifier::new(Arc::new(config.oidc.clone()))
            .context("cannot initialize OIDC verifier")?,
    );
    let state = AppState::new(
        manager.clone(),
        verifier,
        Duration::from_secs(config.server.allocation_wait_seconds),
    );
    // Host-only authority for the operator surface: a forwarder or a browser
    // reaches loopback, but neither can read a 0600 file in the state directory.
    let operator_token = ensure_operator_token(&config.runtime.state_dir)
        .await
        .context("cannot establish the host-only operator credential")?;
    let shutdown = CancellationToken::new();
    let application = build_surfaces(&config, &manager, operator_token, state, &shutdown).await?;
    let listener = TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.server.listen))?;
    let listener = BoundedListener::new(listener, MAX_CONNECTIONS, HEAD_TIMEOUT);
    tracing::info!(listen = %config.server.listen, version = env!("CARGO_PKG_VERSION"), "flanforged started");
    spawn_config_reload(
        config_path.to_path_buf(),
        stamp,
        Arc::clone(&handle),
        shutdown.clone(),
    );
    spawn_reaper(manager.clone(), handle, shutdown.clone());
    let server_shutdown = shutdown.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(
            with_peer_info(listener),
            application.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(server_shutdown.cancelled_owned())
        .await
    });
    tokio::select! {
        result = &mut server => join_server(result),
        () = shutdown_signal() => {
            shutdown.cancel();
            let grace = Duration::from_secs(config.server.shutdown_grace_seconds);
            let graceful = async {
                let (is_clean, server_result) = tokio::join!(manager.shutdown(grace), &mut server);
                if !is_clean {
                    tracing::warn!("allocation cleanup exceeded the shutdown grace period");
                }
                join_server(server_result)
            };
            if let Ok(result) = tokio::time::timeout(grace + Duration::from_secs(1), graceful).await {
                tracing::info!("flanforged shutdown complete");
                result
            } else {
                tracing::warn!("service shutdown exceeded its grace period");
                server.abort();
                Ok(())
            }
        }
    }
}

/// Opens the credential, the durable stores, and the Tart worker the manager
/// runs on. Every failure here is a refusal to start.
async fn open_manager(config: &Config, handle: Arc<ConfigHandle>) -> Result<AllocationManager> {
    let token = read_secret_file(&config.forgejo.api_token_file)
        .await
        .context("cannot read Forgejo credential")?;
    tracing::debug!("Forgejo service credential loaded");
    let forgejo = ForgejoClient::new(Arc::new(config.forgejo.clone()), token)
        .context("cannot initialize Forgejo client")?;
    let store = Arc::new(
        JsonStateStore::open(&config.runtime.state_dir)
            .await
            .context("cannot open allocation state")?,
    );
    let images = Arc::new(
        JsonWarmImageStore::open(&config.runtime.state_dir)
            .await
            .context("cannot open warm image records")?,
    );
    tracing::debug!("allocation state store opened");
    let worker = Arc::new(FlanForgeWorker::new(config, forgejo));
    Ok(AllocationManager::new(handle, store, images, worker))
}

/// Composes the public router and starts the operator surface on whichever
/// listener the listen address calls for.
async fn build_surfaces(
    config: &Config,
    manager: &AllocationManager,
    operator_token: String,
    state: AppState,
    shutdown: &CancellationToken,
) -> Result<axum::Router> {
    let operator = build_operator_router(
        OperatorState::new(manager.clone(), config.server.listen, operator_token),
        config.server.request_body_limit_bytes,
    );
    let application = build_router(state, config.server.request_body_limit_bytes);
    if let Some(operator_listener) = bind_operator_listener(config.server.listen).await? {
        serve_operator(operator_listener, operator, shutdown.clone());
        return Ok(application);
    }
    // A loopback listen address shares its socket with the public surface, so
    // the operator routes stand on their own credential, not on the transport.
    Ok(application.merge(operator))
}

/// Binds a dedicated loopback listener only when the service listens
/// off-loopback; a loopback listen address cannot be bound twice, and already
/// excludes every tailnet peer.
pub(super) async fn bind_operator_listener(listen: SocketAddr) -> Result<Option<TcpListener>> {
    if listen.ip().is_loopback() {
        return Ok(None);
    }
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port());
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on {address}"))?;
    tracing::info!(listen = %address, "operator surface bound to its own loopback listener");
    Ok(Some(listener))
}

fn serve_operator(listener: TcpListener, application: axum::Router, shutdown: CancellationToken) {
    let listener = BoundedListener::new(listener, MAX_CONNECTIONS, HEAD_TIMEOUT);
    tokio::spawn(async move {
        if let Err(error) = axum::serve(
            with_peer_info(listener),
            application.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        {
            tracing::error!(%error, "operator listener stopped");
        }
    });
}

fn join_server(result: Result<Result<(), std::io::Error>, tokio::task::JoinError>) -> Result<()> {
    result
        .context("HTTP server task failed")?
        .context("HTTP server failed")
}
