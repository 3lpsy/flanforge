use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use flanforge_auth::OidcVerifier;
use flanforge_config::{ConfigOverrides, load_config_with_overrides};
use flanforge_core::{Config, RuntimeBackendKind};
use flanforge_forgejo::{ForgejoClient, read_secret_file};
use flanforge_manager::{AllocationManager, AllocationWorker, ConfigHandle, ensure_operator_token};
use flanforge_orm::{
    SqliteAllocationStore, SqliteEventSink, SqliteHotGuestStore, SqliteWarmImageStore,
};
use flanforge_router::{
    BoundedListener, build_operator_router, build_router, build_webui_router, with_peer_info,
};
use flanforge_routes::{AppState, OperatorState};
#[cfg(target_os = "macos")]
use flanforge_runtime::ensure_guest_known_hosts;
#[cfg(target_os = "linux")]
use flanforge_runtime_libvirt::LibvirtWorker;
#[cfg(target_os = "macos")]
use flanforge_runtime_tart::FlanForgeWorker;
use flanforge_store::{BroadcastEventSink, EventSink, StateMutationLock, TeeEventSink};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::{
    signals::{file_stamp, shutdown_signal, spawn_config_reload},
    tasks::spawn_reaper,
};

/// Ingress bounds: far above the one allocation the daemon serves, far below
/// the descriptor limit, and short enough that a stalled peer cannot linger.
const MAX_CONNECTIONS: usize = 128;
pub(crate) const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// A complete stop. Startup and runtime failures exit 1, and clap exits 2 for
/// a usage error, so neither collides with an abandoned teardown.
pub(crate) const EXIT_OK: u8 = 0;

/// Teardown was abandoned: a guest or an ephemeral runner registration may
/// still be live, and the log names each one.
pub(crate) const EXIT_INCOMPLETE_SHUTDOWN: u8 = 3;

/// The status one stop hands the service supervisor. Only an abandoned
/// teardown is non-zero: an ordinary stop must not look failed.
pub(crate) const fn stop_exit_status(is_clean: bool) -> u8 {
    if is_clean {
        EXIT_OK
    } else {
        EXIT_INCOMPLETE_SHUTDOWN
    }
}

/// Names the channel every allocation will be driven over, and says plainly
/// what an SSH table means when it is not the channel in use.
fn report_guest_channel(config: &Config) {
    tracing::info!(channel = %config.guest.channel, "guest control channel");
    if config.guest.channel == flanforge_core::GuestChannelKind::Agent && config.guest.ssh.is_some()
    {
        tracing::warn!(
            "[guest.ssh] is configured while the agent channel is selected: the key is seeded for \
             break-glass access only and sshd is left enabled"
        );
    }
}

/// Risky-but-permitted configuration, logged at every load. The daemon adds no
/// context: `flanforge-core` returns advisories as data because it has no
/// `tracing` and must not gain one.
fn report_advisories(config: &Config) {
    for advisory in config.advisories() {
        tracing::warn!("{}", advisory.message());
    }
}

/// Loads, validates, and reports the startup configuration; returns the
/// pre-read stamp the reload watcher continues from.
async fn load_startup_config(
    config_path: &Path,
    overrides: &ConfigOverrides,
) -> Result<(super::signals::FileStamp, Arc<Config>)> {
    tracing::debug!(config_path = %config_path.display(), "loading service configuration");
    // Stamped before the read: an edit written while recovery runs then differs
    // from it and reaches the watcher instead of being skipped as applied.
    let stamp = file_stamp(config_path).await;
    let config = Arc::new(load_config_with_overrides(config_path, overrides).await?);
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
    report_advisories(&config);
    ensure_target_backend(&config)?;
    report_guest_channel(&config);
    Ok((stamp, config))
}

pub(crate) async fn run_daemon(config_path: &Path, overrides: &ConfigOverrides) -> Result<u8> {
    let (stamp, config) = load_startup_config(config_path, overrides).await?;
    #[cfg(target_os = "macos")]
    ensure_guest_known_hosts(&config.guest)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .context("guest host-key anchor is unusable")?;

    let handle = ConfigHandle::new(Arc::clone(&config));
    // The lock is the daemon's exclusive VM-management authority; held here
    // for the process lifetime, released only when this function returns.
    let (manager, events, event_feed, connection, _state_lock) =
        open_manager(&config, config_path, Arc::clone(&handle)).await?;
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
    let reload_now = Arc::new(tokio::sync::Notify::new());
    let application = build_surfaces(
        &config,
        &handle,
        &manager,
        operator_token,
        state,
        &connection,
        event_feed,
        WebuiPlumbing {
            events: Arc::clone(&events),
            config_path: config_path.to_path_buf(),
            reload_now: Arc::clone(&reload_now),
        },
        &shutdown,
    )
    .await?;
    let listener = TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.server.listen))?;
    let listener = BoundedListener::new(listener, MAX_CONNECTIONS, HEAD_TIMEOUT);
    tracing::info!(listen = %config.server.listen, version = env!("CARGO_PKG_VERSION"), "flanforged started");
    spawn_config_reload(
        config_path.to_path_buf(),
        stamp,
        Arc::clone(&handle),
        overrides.clone(),
        events,
        reload_now,
        shutdown.clone(),
    );
    // A reload that resizes or repoints a profile must drain that profile's
    // pool now, not at the next sweep: `reap_interval_hours` may be a day.
    manager.spawn_hot_reload_watch(shutdown.clone());
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
        result = &mut server => join_server(result).map(|()| EXIT_OK),
        () = shutdown_signal() => {
            shutdown.cancel();
            let grace = Duration::from_secs(config.server.shutdown_grace_seconds);
            // The manager logs one event per leaked allocation; the status only
            // has to tell the supervisor that something was left behind.
            let graceful = async {
                let (report, server_result) = tokio::join!(manager.shutdown(grace), &mut server);
                (report.is_clean(), join_server(server_result))
            };
            if let Ok((is_clean, result)) = tokio::time::timeout(grace + Duration::from_secs(1), graceful).await {
                result?;
                tracing::info!(is_clean, "flanforged shutdown complete");
                Ok(stop_exit_status(is_clean))
            } else {
                tracing::warn!("service shutdown exceeded its grace period; teardown was abandoned");
                server.abort();
                Ok(stop_exit_status(false))
            }
        }
    }
}

/// Opens the credential, the database-backed stores, and the target-native
/// worker the manager runs on. Every failure here is a refusal to start.
async fn open_manager(
    config: &Config,
    config_path: &Path,
    handle: Arc<ConfigHandle>,
) -> Result<(
    AllocationManager,
    Arc<dyn EventSink>,
    Arc<BroadcastEventSink>,
    sea_orm::DatabaseConnection,
    StateMutationLock,
)> {
    let token = read_secret_file(&config.forgejo.api_token_file)
        .await
        .context("cannot read Forgejo credential")?;
    tracing::debug!("Forgejo service credential loaded");
    let forgejo = ForgejoClient::new(Arc::new(config.forgejo.clone()), token)
        .context("cannot initialize Forgejo client")?;
    let lock = StateMutationLock::acquire(&config.runtime.state_dir)
        .await
        .context("cannot lock the state directory")?;
    let db_path = config.db.resolved_db_path(config_path);
    let connection = flanforge_migrations::connect_and_migrate(&db_path)
        .await
        .context("cannot open the daemon database")?;
    flanforge_orm::ensure_state_dir_identity(&connection, &config.runtime.state_dir)
        .await
        .context("the database does not belong to this state directory")?;
    if let Some(report) = flanforge_orm::import_json_state(&connection, &config.runtime.state_dir)
        .await
        .context("cannot import the legacy JSON state")?
    {
        tracing::info!(?report, "legacy JSON state imported into the database");
    }
    match flanforge_orm::prune_history(&connection).await {
        Ok(outcome) => tracing::debug!(?outcome, "startup history retention pass"),
        Err(error) => tracing::warn!(%error, "startup history retention pass failed"),
    }
    tracing::info!(db = %db_path.display(), "daemon database opened");
    // Before the worker probes and long before the listener binds: dropped
    // image artifacts become published bases under the daemon's own lock.
    #[cfg(target_os = "linux")]
    flanforge_runtime_libvirt::auto_import_bases(config, &lock).await;
    let store = Arc::new(SqliteAllocationStore::new(connection.clone()));
    let images = Arc::new(SqliteWarmImageStore::new(connection.clone()));
    let hot = Arc::new(SqliteHotGuestStore::new(connection.clone()));
    // Every event lands durably and, in the same breath, on the live feed
    // the web UI streams from.
    let event_feed = Arc::new(BroadcastEventSink::default());
    let events: Arc<dyn EventSink> = Arc::new(TeeEventSink(vec![
        Arc::new(SqliteEventSink::new(connection.clone())),
        Arc::clone(&event_feed) as Arc<dyn EventSink>,
    ]));
    let worker = open_worker(config, handle.subscribe(), forgejo).await?;
    Ok((
        AllocationManager::new_with_events(handle, store, images, hot, Arc::clone(&events), worker),
        events,
        event_feed,
        connection,
        lock,
    ))
}

async fn open_worker(
    config: &Config,
    config_watch: tokio::sync::watch::Receiver<Arc<Config>>,
    forgejo: ForgejoClient,
) -> Result<Arc<dyn AllocationWorker>> {
    ensure_target_backend(config)?;
    #[cfg(target_os = "macos")]
    {
        let _ = config_watch;
        let worker =
            FlanForgeWorker::try_new(config, forgejo).context("cannot initialize Tart runtime")?;
        Ok(Arc::new(worker))
    }
    #[cfg(target_os = "linux")]
    {
        let worker = LibvirtWorker::open(config_watch, forgejo)
            .await
            .context("cannot initialize libvirt runtime")?;
        Ok(Arc::new(worker))
    }
}

/// Builds the OIDC relying party when `[webui.oidc]` is enabled. A missing
/// or unreadable secret is a refusal to start: the operator asked for SSO.
async fn open_webui_oidc(
    config: &Config,
) -> Result<Option<Arc<flanforge_webui_auth::oidc::OidcRelyingParty>>> {
    let oidc = &config.webui.oidc;
    if !oidc.enabled {
        return Ok(None);
    }
    let (Some(issuer), Some(client_id), Some(secret_file), Some(redirect_url)) = (
        oidc.issuer.clone(),
        oidc.client_id.clone(),
        oidc.client_secret_file.as_deref(),
        oidc.redirect_url.clone(),
    ) else {
        anyhow::bail!("webui.oidc is enabled but incompletely configured");
    };
    let client_secret = read_secret_file(secret_file)
        .await
        .context("cannot read the webui OIDC client secret")?;
    let relying_party = flanforge_webui_auth::oidc::OidcRelyingParty::new(
        flanforge_webui_auth::oidc::OidcRpConfig {
            issuer,
            client_id,
            client_secret,
            redirect_url,
            scopes: oidc.scopes.clone(),
        },
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
    .context("cannot initialize the webui OIDC relying party")?;
    tracing::info!("webui OIDC login enabled");
    Ok(Some(Arc::new(relying_party)))
}

pub(super) fn ensure_target_backend(config: &Config) -> Result<()> {
    let configured = config.runtime.backend_kind();
    let expected = RuntimeBackendKind::default();
    anyhow::ensure!(
        configured == expected,
        "runtime backend {configured} is unavailable on this target; expected {expected}"
    );
    Ok(())
}

/// Composes the public router and starts the operator surface on whichever
/// listener the listen address calls for.
/// What the webui's config-edit path needs from the daemon's wiring.
struct WebuiPlumbing {
    events: Arc<dyn EventSink>,
    config_path: std::path::PathBuf,
    reload_now: Arc<tokio::sync::Notify>,
}

#[allow(clippy::too_many_arguments)]
async fn build_surfaces(
    config: &Config,
    handle: &Arc<ConfigHandle>,
    manager: &AllocationManager,
    operator_token: String,
    state: AppState,
    connection: &sea_orm::DatabaseConnection,
    event_feed: Arc<BroadcastEventSink>,
    plumbing: WebuiPlumbing,
    shutdown: &CancellationToken,
) -> Result<axum::Router> {
    let operator = build_operator_router(
        OperatorState::new(
            manager.clone(),
            config.server.listen,
            operator_token,
            flanforge_orm::UserService::new(connection.clone()),
            flanforge_orm::SessionService::new(connection.clone()),
        ),
        config.server.request_body_limit_bytes,
    );
    let mut application = build_router(state, config.server.request_body_limit_bytes);
    // The browser surface rides the public router behind its own session
    // auth; the webui branch carries its own, larger body limit.
    if config.webui.enabled {
        let services = flanforge_handlers::WebuiServices {
            users: flanforge_orm::UserService::new(connection.clone()),
            sessions: flanforge_orm::SessionService::new(connection.clone()),
            history: flanforge_orm::HistoryService::new(connection.clone()),
            manager: manager.clone(),
            config: Arc::clone(handle),
            event_feed,
            oidc: open_webui_oidc(config).await?,
            events: plumbing.events,
            config_path: plumbing.config_path,
            reload_now: plumbing.reload_now,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        application = application.merge(build_webui_router(
            services,
            config.webui.request_body_limit_bytes,
            config.webui.dev_dist_dir.clone(),
        ));
        tracing::info!(
            public_read_only = config.webui.public_read_only,
            authdb = config.webui.authdb.enabled,
            oidc = config.webui.oidc.enabled,
            "web UI enabled at /ui"
        );
    }
    if let Some(operator_listener) = bind_operator_listener(config.server.listen).await? {
        serve_operator(operator_listener, operator, shutdown.clone());
        return Ok(application);
    }
    // A loopback listen address shares its socket with the public surface, so
    // the operator routes stand on their own credential, not on the transport.
    Ok(application.merge(operator))
}

/// Binds a dedicated loopback listener only when the service listens on one
/// specific off-loopback address; that address cannot also be bound here, and
/// a loopback listen address already excludes every remote peer.
///
/// A wildcard address covers loopback itself, so the same port cannot be bound
/// twice and the operator routes are served on the public socket instead. They
/// carry their own credential, which is what protects them either way.
pub(super) async fn bind_operator_listener(listen: SocketAddr) -> Result<Option<TcpListener>> {
    if listen.ip().is_loopback() {
        return Ok(None);
    }
    if listen.ip().is_unspecified() {
        tracing::warn!(
            listen = %listen,
            "a wildcard listen address serves the operator surface on the public socket; it is reachable wherever this binds, behind its credential"
        );
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
