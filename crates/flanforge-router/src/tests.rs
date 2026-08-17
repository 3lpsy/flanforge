use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{
        Request, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN},
    },
};
use flanforge_core::{Allocation, AllocationState, ForgejoClaims, Profile};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use flanforge_auth::{AuthError, TokenVerifier};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationSummary, AllocationWorker, ConfigHandle,
    OPERATOR_TOKEN_HEADER, WorkerError,
};
use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore};
use flanforge_test_support as test_support;

use flanforge_routes::{AppState, OperatorState};

use super::build_router;

#[derive(Debug)]
struct StaticVerifier;

#[async_trait]
impl TokenVerifier for StaticVerifier {
    async fn verify(&self, token: &str) -> Result<ForgejoClaims, AuthError> {
        if token == "valid.token.signature" {
            Ok(test_support::claims())
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}

#[derive(Debug)]
struct ReadyWorker;

#[async_trait]
impl AllocationWorker for ReadyWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        for state in [
            AllocationState::Preparing,
            AllocationState::Booting,
            AllocationState::Registering,
            AllocationState::WaitingForJob,
        ] {
            reporter
                .transition(state)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
        }
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        Ok(())
    }
}

#[derive(Debug)]
struct TimeoutWorker {
    cleaned: Arc<AtomicBool>,
}

#[async_trait]
impl AllocationWorker for TimeoutWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        reporter
            .transition(AllocationState::Preparing)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        self.cleaned.store(true, Ordering::SeqCst);
        Ok(())
    }
}

async fn application() -> (axum::Router, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(config),
        store,
        images,
        Arc::new(ReadyWorker),
    );
    let state = AppState::new(manager, Arc::new(StaticVerifier), Duration::from_secs(2));
    (build_router(state, 4_096), directory)
}

#[tokio::test]
async fn health_is_public_and_minimal() {
    let (app, _directory) = application().await;
    let response = app
        .oneshot(
            Request::get("/healthz")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn allocation_requires_authentication() {
    let (app, _directory) = application().await;
    let response = app
        .oneshot(allocation_request(None))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn allocation_returns_the_unique_runner_label_before_job_queueing() {
    let (app, _directory) = application().await;
    let response = app
        .oneshot(allocation_request(Some("Bearer valid.token.signature")))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let allocation: Allocation =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(allocation.state, AllocationState::WaitingForJob);
    assert!(
        allocation
            .runner_label
            .as_str()
            .starts_with("macos-tart-project-")
    );
}

#[tokio::test]
async fn allocation_timeout_cancels_and_cleans_the_worker() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let cleaned = Arc::new(AtomicBool::new(false));
    let worker = Arc::new(TimeoutWorker {
        cleaned: cleaned.clone(),
    });
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(ConfigHandle::new(config), store, images, worker);
    let state = AppState::new(manager, Arc::new(StaticVerifier), Duration::from_millis(20));
    let app = build_router(state, 4_096);
    let response = app
        .oneshot(allocation_request(Some("Bearer valid.token.signature")))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    // The budget only has to exceed cleanup, not measure it: a tight bound
    // fails under a loaded test run rather than on a real defect.
    for _ in 0..500 {
        if cleaned.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    unreachable!("timed-out allocation was not cleaned");
}

#[tokio::test]
async fn allocation_rejects_structurally_hostile_workflow_fields() {
    for body in [
        r#"{"profile":"project;touch","repository":"owner/project","run_id":"42","run_attempt":1}"#,
        r#"{"profile":"project","repository":"owner/project$(id)","run_id":"42","run_attempt":1}"#,
        r#"{"profile":"project","repository":"owner/project","run_id":"42;touch","run_attempt":1}"#,
        r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":0}"#,
        r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":1,"command":"touch /tmp/pwned"}"#,
    ] {
        let (app, _directory) = application().await;
        let response = app
            .oneshot(allocation_request_with_body(
                Some("Bearer valid.token.signature"),
                body,
            ))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "body: {body}");
    }
}

#[tokio::test]
async fn over_ceiling_request_is_403_and_the_body_discloses_nothing() {
    let (app, _directory) = application().await;
    let response = app
        .oneshot(allocation_request_with_body(
            Some("Bearer valid.token.signature"),
            r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":1,"cpu_count":64}"#,
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    assert_eq!(
        String::from_utf8_lossy(&body),
        r#"{"error":"request is not authorized"}"#
    );
}

#[tokio::test]
async fn a_request_beneath_the_ceiling_records_its_sizing() {
    let (app, _directory) = application().await;
    let response = app
        .oneshot(allocation_request_with_body(
            Some("Bearer valid.token.signature"),
            r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":1,"warm":true,"cpu_count":2}"#,
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let allocation: Allocation =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(
        allocation.size,
        Some(flanforge_core::GuestSize {
            cpu_count: 2,
            memory_mb: 8_192,
        })
    );
    assert_eq!(allocation.mode, flanforge_core::AllocationMode::Warm);
}

fn allocation_request(authorization: Option<&str>) -> Request<Body> {
    allocation_request_with_body(
        authorization,
        r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":1}"#,
    )
}

fn allocation_request_with_body(authorization: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::post("/v1/allocations").header(CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(AUTHORIZATION, value);
    }
    builder
        .body(Body::from(body.to_owned()))
        .unwrap_or_else(|error| unreachable!("request: {error}"))
}

#[tokio::test]
async fn the_listener_bounds_connections_and_drops_an_incomplete_request_head() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (application, _directory) = application().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let bounded = super::BoundedListener::new(listener, 1, Duration::from_millis(200));
    tokio::spawn(async move {
        let _ = axum::serve(bounded, application).await;
    });

    let mut stalled = tokio::net::TcpStream::connect(address)
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    stalled
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap_or_else(|error| unreachable!("write: {error}"));
    let mut discarded = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), stalled.read_to_end(&mut discarded))
        .await
        .unwrap_or_else(|_| unreachable!("stalled connection was never closed"));
    assert!(closed.map_or(true, |read| read == 0));

    let mut healthy = tokio::net::TcpStream::connect(address)
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    healthy
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap_or_else(|error| unreachable!("write: {error}"));
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), healthy.read_to_end(&mut response))
        .await
        .unwrap_or_else(|_| unreachable!("the freed connection slot was never reused"))
        .unwrap_or_else(|error| unreachable!("read: {error}"));
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
}

async fn operator_application() -> (axum::Router, AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        Arc::new(ReadyWorker),
    );
    let state = OperatorState::new(
        manager.clone(),
        config.server.listen,
        OPERATOR_TOKEN.to_owned(),
    );
    (
        super::build_operator_router(state, 4_096),
        manager,
        directory,
    )
}

/// The credential the daemon writes 0600 into its state directory.
const OPERATOR_TOKEN: &str = "5f2c0a0f5f2c0a0f5f2c0a0f5f2c0a0f";

/// The address the fixture configuration binds, and so the only `Host` the
/// operator surface accepts.
fn listen() -> SocketAddr {
    flanforge_core::ServerConfig::default().listen
}

fn from_peer(mut request: Request<Body>, peer: &str) -> Request<Body> {
    let peer: SocketAddr = peer
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

/// A request shaped exactly as the operator CLI sends it.
fn as_operator(request: Request<Body>, peer: &str, listen: SocketAddr) -> Request<Body> {
    let mut request = from_peer(request, peer);
    let headers = request.headers_mut();
    headers.insert(
        HOST,
        listen
            .to_string()
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    headers.insert(
        OPERATOR_TOKEN_HEADER,
        OPERATOR_TOKEN
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    request
}

fn operator_probes() -> Vec<Request<Body>> {
    vec![
        Request::get("/v1/operator/allocations")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}")),
        Request::delete("/v1/operator/allocations/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}")),
        Request::get("/v1/operator/status")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}")),
        Request::post("/v1/operator/reap")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}")),
    ]
}

#[tokio::test]
async fn every_operator_route_rejects_a_non_local_peer() {
    for probe in operator_probes() {
        let (app, _manager, _directory) = operator_application().await;
        let response = app
            .oneshot(from_peer(probe, "100.64.0.9:51000"))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // A route added to the surface after the fact is covered by the same check.
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        Arc::new(ReadyWorker),
    );
    let state = OperatorState::new(manager, config.server.listen, OPERATOR_TOKEN.to_owned());
    let app = super::operator_surface(
        super::operator_routes().route(
            "/v1/operator/added-later",
            axum::routing::get(|| async { "added" }),
        ),
        state,
        4_096,
    );
    let response = app
        .oneshot(from_peer(
            Request::get("/v1/operator/added-later")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
            "100.64.0.9:51000",
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn no_operator_route_creates_an_allocation_or_alters_a_profile() {
    for (method, path) in [
        ("POST", "/v1/operator/allocations"),
        ("PUT", "/v1/operator/allocations"),
        ("POST", "/v1/operator/profiles"),
        ("GET", "/v1/operator/profiles"),
        ("PUT", "/v1/operator/status"),
    ] {
        let (app, _manager, _directory) = operator_application().await;
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}"));
        let response = app
            .oneshot(as_operator(request, "127.0.0.1:51000", listen()))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert!(
            matches!(
                response.status(),
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{method} {path} is mounted"
        );
    }

    // The allocation surface is equally absent from the operator listener.
    let (app, _manager, _directory) = operator_application().await;
    let response = app
        .oneshot(as_operator(
            allocation_request(Some("Bearer valid.token.signature")),
            "127.0.0.1:51000",
            listen(),
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_listing_reflects_an_http_created_allocation() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        Arc::new(ReadyWorker),
    );
    // The loopback shape: one listener serving both surfaces.
    let application = super::build_router(
        AppState::new(
            manager.clone(),
            Arc::new(StaticVerifier),
            Duration::from_secs(2),
        ),
        4_096,
    )
    .merge(super::build_operator_router(
        OperatorState::new(manager, config.server.listen, OPERATOR_TOKEN.to_owned()),
        4_096,
    ));

    let response = application
        .clone()
        .oneshot(allocation_request(Some("Bearer valid.token.signature")))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let created: Allocation =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));

    let listed = application
        .oneshot(as_operator(
            Request::get("/v1/operator/allocations")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
            "127.0.0.1:51000",
            config.server.listen,
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(listed.status(), StatusCode::OK);
    let body = to_bytes(listed.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let summaries: Vec<AllocationSummary> =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, created.id);
    assert_eq!(summaries[0].repository, created.request.repository);
    assert_eq!(summaries[0].run_id, created.request.run_id);
}

/// SEC-580 / SEC-581: a loopback peer is not an operator. A forwarder
/// re-originates every tailnet caller from loopback, and a page in a browser is
/// loopback too; only the host-only credential and the bound authority separate
/// them from a shell on this machine.
#[tokio::test]
async fn every_operator_route_refuses_a_loopback_request_that_is_not_the_cli() {
    for probe in operator_probes() {
        let (app, _manager, _directory) = operator_application().await;
        // The forwarded caller: loopback peer, right Host, no credential.
        let mut forwarded = from_peer(probe, "127.0.0.1:51000");
        forwarded.headers_mut().insert(
            HOST,
            listen()
                .to_string()
                .parse()
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        let response = app
            .oneshot(forwarded)
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    for probe in operator_probes() {
        // The rebound page: the credential is still unreadable to it, and the
        // name it was served from is not the authority the daemon bound.
        let (app, _manager, _directory) = operator_application().await;
        let mut rebound = as_operator(probe, "127.0.0.1:51000", listen());
        rebound.headers_mut().insert(
            HOST,
            "flanforge.example:9843"
                .parse()
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        let response = app
            .oneshot(rebound)
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    for probe in operator_probes() {
        // Anything a browser originates carries an Origin, credential or not.
        let (app, _manager, _directory) = operator_application().await;
        let mut origin = as_operator(probe, "127.0.0.1:51000", listen());
        origin.headers_mut().insert(
            ORIGIN,
            "http://flanforge.example"
                .parse()
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        let response = app
            .oneshot(origin)
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    for probe in operator_probes() {
        // A wrong credential is no better than none.
        let (app, _manager, _directory) = operator_application().await;
        let mut wrong = as_operator(probe, "127.0.0.1:51000", listen());
        wrong.headers_mut().insert(
            OPERATOR_TOKEN_HEADER,
            "0000000000000000000000000000000f"
                .parse()
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        let response = app
            .oneshot(wrong)
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // The CLI's own shape still works.
    let (app, _manager, _directory) = operator_application().await;
    let response = app
        .oneshot(as_operator(
            Request::get("/v1/operator/status")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
            "127.0.0.1:51000",
            listen(),
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);
}
