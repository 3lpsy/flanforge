use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use flanforge_core::{Allocation, ForgejoClaims, Profile};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
    WorkerError,
};
use flanforge_router::{build_operator_router, build_router};
use flanforge_routes::{AppState, OperatorState};
use flanforge_test_support as test_support;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use flanforge_auth::{AuthError, TokenRejection, TokenVerifier};

use super::run::bind_operator_listener;
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

#[derive(Debug)]
struct IdleWorker;

#[async_trait]
impl AllocationWorker for IdleWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

#[derive(Debug)]
struct RejectingVerifier;

#[async_trait]
impl TokenVerifier for RejectingVerifier {
    async fn verify(&self, _token: &str) -> Result<ForgejoClaims, AuthError> {
        Err(AuthError::InvalidToken(TokenRejection::Signature))
    }
}

/// A worker whose teardown outlives any shutdown grace a test hands it, so a
/// stop has to abandon it.
#[derive(Debug)]
struct StuckWorker;

#[async_trait]
impl AllocationWorker for StuckWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        tokio::time::sleep(Duration::from_mins(1)).await;
        Ok(())
    }
}

async fn manager_with(worker: Arc<dyn AllocationWorker>) -> (AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("state.db"))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(SqliteAllocationStore::new(connection.clone()));
    let images = Arc::new(SqliteWarmImageStore::new(connection.clone()));
    let hot = Arc::new(SqliteHotGuestStore::new(connection));
    (
        AllocationManager::new(ConfigHandle::new(config), store, images, hot, worker),
        directory,
    )
}

/// RUN-202: an abandoned teardown is a failed unit; an ordinary stop is not.
#[tokio::test]
async fn an_abandoned_teardown_exits_non_zero_and_a_clean_stop_exits_zero() {
    // StuckWorker never finishes, so a short grace proves abandonment without
    // waiting. IdleWorker must finish *inside* its grace, so that one is
    // generous: a wall-clock second is missed on a loaded host and the test
    // would fail on machine speed rather than on behaviour.
    const ABANDONED_GRACE: Duration = Duration::from_secs(1);
    const CLEAN_GRACE: Duration = Duration::from_secs(30);
    let (stuck, _stuck_directory) = manager_with(Arc::new(StuckWorker)).await;
    stuck
        .create(
            test_support::request(),
            flanforge_core::RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    let abandoned = stuck.shutdown(ABANDONED_GRACE).await;

    assert!(!abandoned.is_clean());
    assert_eq!(
        super::stop_exit_status(abandoned.is_clean()),
        super::EXIT_INCOMPLETE_SHUTDOWN
    );

    let (idle, _idle_directory) = manager_with(Arc::new(IdleWorker)).await;
    idle.create(
        test_support::request(),
        flanforge_core::RequestOptions::default(),
        &test_support::claims(),
    )
    .await
    .unwrap_or_else(|error| unreachable!("create: {error}"));

    let complete = idle.shutdown(CLEAN_GRACE).await;

    assert!(complete.is_clean(), "{:?}", complete.leaked());
    assert_eq!(super::stop_exit_status(complete.is_clean()), super::EXIT_OK);
}

/// The two routers exactly as `run_daemon` composes them for an off-loopback
/// listen address.
async fn split_listeners(listen: SocketAddr) -> (axum::Router, axum::Router, tempfile::TempDir) {
    let (manager, directory) = manager_with(Arc::new(IdleWorker)).await;
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("state.db"))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let state = AppState::new(
        manager.clone(),
        Arc::new(RejectingVerifier),
        Duration::from_secs(1),
    );
    (
        build_router(state, 4_096),
        build_operator_router(
            OperatorState::new(
                manager,
                listen,
                OPERATOR_TOKEN.to_owned(),
                flanforge_orm::UserService::new(connection.clone()),
                flanforge_orm::SessionService::new(connection),
            ),
            4_096,
        ),
        directory,
    )
}

/// The credential the daemon writes 0600 into its state directory.
const OPERATOR_TOKEN: &str = "5f2c0a0f5f2c0a0f5f2c0a0f5f2c0a0f";

fn from_loopback(request: Request<Body>) -> Request<Body> {
    let mut request = request;
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        51_000,
    )));
    request
}

/// A request shaped exactly as the operator CLI sends it.
fn as_operator(request: Request<Body>, listen: SocketAddr) -> Request<Body> {
    let mut request = from_loopback(request);
    let headers = request.headers_mut();
    headers.insert(
        axum::http::header::HOST,
        listen
            .to_string()
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    headers.insert(
        flanforge_manager::OPERATOR_TOKEN_HEADER,
        OPERATOR_TOKEN
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    request
}

#[tokio::test]
async fn a_loopback_listen_address_needs_no_second_listener() {
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9_843);
    assert!(
        bind_operator_listener(loopback)
            .await
            .unwrap_or_else(|error| unreachable!("bind: {error}"))
            .is_none()
    );
}

/// A wildcard bind covers loopback, so claiming `127.0.0.1:<port>` first would
/// make the public bind fail with EADDRINUSE and the daemon never start.
#[tokio::test]
async fn a_wildcard_listen_address_needs_no_second_listener() {
    for wildcard in [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9_843),
        SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 9_843),
    ] {
        assert!(
            bind_operator_listener(wildcard)
                .await
                .unwrap_or_else(|error| unreachable!("bind: {error}"))
                .is_none(),
            "{wildcard} bound a second listener"
        );
    }
}

#[tokio::test]
async fn the_operator_router_is_absent_from_the_off_loopback_listener() {
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)), 0);
    let bound = bind_operator_listener(listen)
        .await
        .unwrap_or_else(|error| unreachable!("bind: {error}"));
    let bound = bound.unwrap_or_else(|| unreachable!("an off-loopback listen address binds one"));
    assert!(
        bound
            .local_addr()
            .unwrap_or_else(|error| unreachable!("addr: {error}"))
            .ip()
            .is_loopback()
    );

    let (tailnet, operator, _directory) = split_listeners(listen).await;
    for path in [
        "/v1/operator/allocations",
        "/v1/operator/status",
        "/v1/operator/reap",
        "/v1/operator/hot",
    ] {
        let response = tailnet
            .clone()
            .oneshot(from_loopback(
                Request::get(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            ))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} is reachable from the tailnet listener"
        );
    }

    // The tailnet listener still serves the public surface.
    let health = tailnet
        .oneshot(from_loopback(
            Request::get("/healthz")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(health.status(), StatusCode::OK);

    let operator_listing = operator
        .oneshot(as_operator(
            Request::get("/v1/operator/allocations")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
            listen,
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(operator_listing.status(), StatusCode::OK);
}

/// Counts host listings, so a pass the reaper should not have made is visible.
/// A deleting sweep with no candidates lists exactly once: the prune and the
/// retirement both reason over the listing that pass already read.
#[derive(Debug, Default)]
struct CountingWorker {
    listings: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl AllocationWorker for CountingWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<flanforge_manager::HostMachine>, WorkerError> {
        self.listings
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(Vec::new())
    }
}

const LISTINGS_PER_SWEEP: usize = 1;

async fn reaper_fixture(
    hours: u64,
) -> (
    AllocationManager,
    Arc<ConfigHandle>,
    Arc<CountingWorker>,
    Arc<flanforge_core::Config>,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.reap_interval_hours = hours;
    let config = Arc::new(config);
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("state.db"))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let store = Arc::new(SqliteAllocationStore::new(connection.clone()));
    let images = Arc::new(SqliteWarmImageStore::new(connection.clone()));
    let hot = Arc::new(SqliteHotGuestStore::new(connection));
    let worker = Arc::new(CountingWorker::default());
    let handle = ConfigHandle::new(Arc::clone(&config));
    let manager = AllocationManager::new(Arc::clone(&handle), store, images, hot, worker.clone());
    (manager, handle, worker, config, directory)
}

/// Keeps the runtime busy so the paused clock never auto-advances past the
/// point a test is still setting up. The bound stands in for wall time, so it
/// has to survive a sweep whose blocking-pool work is scheduled slowly under a
/// fully parallel test run.
async fn until_sweeps(worker: &CountingWorker, sweeps: usize) {
    for spins in 0..2_000_000_u64 {
        if spins.is_multiple_of(10_000) {
            // Let the paused clock creep so sqlx-internal timers can fire
            // without auto-advance jumping whole sleeps at once.
            tokio::time::advance(std::time::Duration::from_millis(1)).await;
        }
        if worker.listings.load(std::sync::atomic::Ordering::Acquire) >= sweeps * LISTINGS_PER_SWEEP
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    unreachable!("the reaper did not sweep");
}

/// RUN-501: a disable written during the sleep stops the sweep instead of
/// costing one more destructive pass.
#[tokio::test]
async fn disabling_the_sweep_prevents_the_next_pass() {
    // The database opens under real time: sqlx's acquire timeout races a
    // paused clock's auto-advance. Only the sweep schedule needs pausing.
    let (manager, handle, worker, config, _directory) = reaper_fixture(168).await;
    tokio::time::pause();
    super::tasks::spawn_reaper(manager, Arc::clone(&handle), CancellationToken::new());
    until_sweeps(&worker, 1).await;

    let mut disabled = (*config).clone();
    disabled.runtime.reap_interval_hours = 0;
    handle
        .apply(Arc::new(disabled))
        .unwrap_or_else(|error| unreachable!("apply: {error}"));
    tokio::time::advance(std::time::Duration::from_hours(200)).await;
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }

    assert_eq!(
        worker.listings.load(std::sync::atomic::Ordering::Acquire),
        LISTINGS_PER_SWEEP
    );
}

/// RUN-501: a shortened interval takes effect without waiting out the old one.
#[tokio::test]
async fn shortening_the_interval_does_not_wait_out_the_old_sleep() {
    let (manager, handle, worker, config, _directory) = reaper_fixture(168).await;
    tokio::time::pause();
    super::tasks::spawn_reaper(manager, Arc::clone(&handle), CancellationToken::new());
    until_sweeps(&worker, 1).await;

    let mut shortened = (*config).clone();
    shortened.runtime.reap_interval_hours = 1;
    handle
        .apply(Arc::new(shortened))
        .unwrap_or_else(|error| unreachable!("apply: {error}"));
    tokio::time::advance(std::time::Duration::from_hours(1)).await;
    until_sweeps(&worker, 2).await;
}

/// CORE-503: the stamp describes what was decoded, so a write that lands during
/// the reload is seen on the next tick instead of being recorded as applied.
#[tokio::test]
async fn a_write_during_a_reload_is_not_stamped_as_applied() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    let one_profile = flanforge_config::STARTER_CONFIG;
    let two_profiles = format!("{one_profile}\n# an edit that lands during the reload\n");
    tokio::fs::write(&path, one_profile)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let (stamp, decoded) =
        super::signals::read_stamped(&path, &flanforge_config::ConfigOverrides::default()).await;
    assert!(decoded.is_ok(), "{decoded:?}");
    tokio::fs::write(&path, &two_profiles)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_ne!(
        stamp,
        super::signals::file_stamp(&path).await,
        "the next tick must still see a change"
    );
}

/// CORE-503: the startup stamp is taken before the startup load, so an edit
/// written while recovery runs is not skipped.
#[tokio::test]
async fn an_edit_written_during_startup_is_still_picked_up() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, flanforge_config::STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // Exactly the order `run_daemon` uses: stamp, then load, then recover.
    let stamp = super::signals::file_stamp(&path).await;
    let _config = flanforge_config::load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    tokio::fs::write(
        &path,
        format!(
            "{}\n# edited while recovery ran\n",
            flanforge_config::STARTER_CONFIG
        ),
    )
    .await
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_ne!(stamp, super::signals::file_stamp(&path).await);
}

#[test]
fn daemon_accepts_only_the_target_native_backend() {
    #[cfg(target_os = "linux")]
    let (native, foreign) = (
        flanforge_config::LIBVIRT_STARTER_CONFIG,
        flanforge_config::STARTER_CONFIG,
    );
    #[cfg(target_os = "macos")]
    let (native, foreign) = (
        flanforge_config::STARTER_CONFIG,
        flanforge_config::LIBVIRT_STARTER_CONFIG,
    );
    let native =
        toml::from_str(native).unwrap_or_else(|error| unreachable!("native fixture: {error}"));
    let foreign =
        toml::from_str(foreign).unwrap_or_else(|error| unreachable!("foreign fixture: {error}"));
    assert!(super::run::ensure_target_backend(&native).is_ok());
    assert!(super::run::ensure_target_backend(&foreign).is_err());
}

#[tokio::test]
async fn stamped_reloads_reapply_the_startup_cli_layer() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, flanforge_config::STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let overrides = flanforge_config::ConfigOverrides::new([("logging.level", "trace")]);
    let (_, config) = super::signals::read_stamped(&path, &overrides).await;
    assert_eq!(
        config
            .unwrap_or_else(|error| unreachable!("reload: {error}"))
            .logging
            .level,
        "trace"
    );
}

/// `BoundedListener` reaps any connection with a write gap past the head
/// deadline; every SSE stream must tick faster than that.
#[test]
fn sse_keep_alive_beats_the_listener_head_deadline() {
    assert!(flanforge_webui_routes::SSE_KEEP_ALIVE < super::run::HEAD_TIMEOUT);
}
