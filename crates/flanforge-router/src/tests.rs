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

use flanforge_auth::{AuthError, TokenRejection, TokenVerifier};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationSummary, AllocationWorker, CleanupBudget,
    ConfigHandle, OPERATOR_TOKEN_HEADER, WorkerError,
};
use flanforge_store::AllocationStore;
use flanforge_test_support as test_support;

use flanforge_routes::{AppState, OperatorState};

use super::build_router;
use flanforge_orm::{
    SessionService, SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore, UserService,
};

/// Operator-state services over the same test database file.
async fn webui_services(directory: &tempfile::TempDir) -> (UserService, SessionService) {
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("state.db"))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    (
        UserService::new(connection.clone()),
        SessionService::new(connection),
    )
}

#[derive(Debug)]
struct StaticVerifier;

#[async_trait]
impl TokenVerifier for StaticVerifier {
    async fn verify(&self, token: &str) -> Result<ForgejoClaims, AuthError> {
        if token == "valid.token.signature" {
            Ok(test_support::claims())
        } else {
            Err(AuthError::InvalidToken(TokenRejection::Signature))
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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

/// Stops the way a host that stayed full for the whole bounded wait does.
#[derive(Debug)]
struct CapacityWorker;

#[async_trait]
impl AllocationWorker for CapacityWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::capacity("Tart VM capacity is unavailable"))
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

/// Stops the way a genuinely broken allocation does.
#[derive(Debug)]
struct FailingWorker;

#[async_trait]
impl AllocationWorker for FailingWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::new("cannot start Tart VM"))
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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.cleaned.store(true, Ordering::SeqCst);
        Ok(())
    }
}

async fn application() -> (axum::Router, tempfile::TempDir) {
    application_with(Arc::new(ReadyWorker)).await
}

async fn application_with(worker: Arc<dyn AllocationWorker>) -> (axum::Router, tempfile::TempDir) {
    let (state, directory) = allocation_state_with(worker).await;
    (build_router(state, 4_096), directory)
}

async fn allocation_state() -> (AppState, tempfile::TempDir) {
    allocation_state_with(Arc::new(ReadyWorker)).await
}

async fn allocation_state_with(worker: Arc<dyn AllocationWorker>) -> (AppState, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(ConfigHandle::new(config), store, images, hot, worker);
    // The wait only bounds how long the route blocks for a ready guest, and
    // ReadyWorker transitions immediately. Two seconds is wall-clock, so a
    // loaded machine misses it and the route answers something other than
    // CREATED — a flake, not a finding.
    let state = AppState::new(manager, Arc::new(StaticVerifier), Duration::from_secs(30));
    (state, directory)
}

/// ARCH-260: `/healthz` is the whole public allowlist, and it answers with no
/// credential at all. Pinning it keeps the opt-out branch from drifting shut.
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
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let health: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(health["status"], "ok");
}

/// Every method and path the allocation surface serves that is not on the
/// public allowlist.
const PROTECTED_PROBES: [(&str, &str); 3] = [
    ("POST", "/v1/allocations"),
    (
        "GET",
        "/v1/allocations/00000000-0000-0000-0000-000000000000",
    ),
    (
        "DELETE",
        "/v1/allocations/00000000-0000-0000-0000-000000000000",
    ),
];

/// The two shapes an unauthenticated caller can present.
const ABSENT_OR_MALFORMED: [Option<&str>; 2] = [None, Some("Bearer malformed-token")];

/// ARCH-260: authentication is the router's default, so every route outside the
/// public allowlist refuses both shapes — not only the one route whose handler
/// happened to remember to check.
#[tokio::test]
async fn every_protected_route_refuses_a_missing_or_malformed_credential() {
    // No probe reaches the manager, so one fixture serves all of them.
    let (app, _directory) = application().await;
    for (method, path) in PROTECTED_PROBES {
        for authorization in ABSENT_OR_MALFORMED {
            let response = app
                .clone()
                .oneshot(probe(method, path, authorization))
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} {authorization:?}"
            );
        }
    }
}

/// ARCH-260: the guarantee is structural rather than a list. A route registered
/// on the protected branch after the fact is authenticated by the same
/// middleware, and the shared layers still reach it, so registration order is
/// no longer load-bearing.
#[tokio::test]
async fn a_route_added_to_the_protected_branch_is_authenticated_and_layered() {
    let (state, _directory) = allocation_state().await;
    let app = super::allocation_surface(
        super::public_routes(),
        super::protected_routes().route(
            "/v1/added-later",
            axum::routing::post(|_: axum::body::Bytes| async { "added" }),
        ),
        state,
        4_096,
    );
    for authorization in ABSENT_OR_MALFORMED {
        let response = app
            .clone()
            .oneshot(probe("POST", "/v1/added-later", authorization))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{authorization:?}"
        );
    }

    // The configured body limit reaches it as well, so it is not only the
    // authentication layer that survived late registration.
    let response = app
        .oneshot(
            Request::post("/v1/added-later")
                .header(AUTHORIZATION, "Bearer valid.token.signature")
                .body(Body::from("x".repeat(8_192)))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// ARCH-260: opting a route out is not a silent bypass. A claims-taking handler
/// mounted on the public branch refuses even a valid credential, because only
/// the middleware verifies one — so a misplaced route fails closed.
#[tokio::test]
async fn a_claims_taking_handler_on_the_public_branch_refuses_a_valid_credential() {
    let (state, _directory) = allocation_state().await;
    let app = super::allocation_surface(
        super::public_routes().route(
            "/v1/misplaced",
            axum::routing::post(flanforge_routes::create),
        ),
        super::protected_routes(),
        state,
        4_096,
    );
    let response = app
        .oneshot(probe(
            "POST",
            "/v1/misplaced",
            Some("Bearer valid.token.signature"),
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// ARCH-260 put verification ahead of the body extractors, closing the
/// unauthenticated validation oracle SEC-262 described: an anonymous caller is
/// answered 401 whatever its body says, never 400.
#[tokio::test]
async fn an_unauthenticated_request_is_refused_before_its_body_is_read() {
    let (app, _directory) = application().await;
    let oversized = "x".repeat(8_192);
    for body in [
        r#"{"profile":"","repository":"nope","run_id":"x","run_attempt":0}"#,
        oversized.as_str(),
    ] {
        let response = app
            .clone()
            .oneshot(allocation_request_with_body(None, body))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

// ARCH-613 names the failing check in the log only: four distinct rejections
// must still be one status, one body, and one header set to the caller.
#[tokio::test]
async fn allocation_requires_authentication() {
    for authorization in [
        None,
        Some("Basic dXNlcjpwYXNz"),
        Some("Bearer one.two"),
        Some("Bearer wrong.token.signature"),
    ] {
        let (app, _directory) = application().await;
        let response = app
            .oneshot(allocation_request(authorization))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{authorization:?}"
        );
        let headers: Vec<String> = response
            .headers()
            .keys()
            .map(|name| name.as_str().to_owned())
            .collect();
        assert_eq!(
            headers,
            ["content-type", "content-length"],
            "{authorization:?}"
        );
        let body = to_bytes(response.into_body(), 16_384)
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"));
        assert_eq!(
            String::from_utf8_lossy(&body),
            r#"{"error":"authentication required"}"#,
            "{authorization:?}"
        );
    }
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

/// ARCH-307: exhausted host capacity used to render as a 502, indistinguishable
/// from a broken build. The allocator job has to be able to tell "retry later"
/// from "this build is broken", so the two codes must stay apart.
#[tokio::test]
async fn a_capacity_exhausted_allocation_is_busy_and_a_failed_one_is_a_gateway_error() {
    for (worker, expected) in [
        (
            Arc::new(CapacityWorker) as Arc<dyn AllocationWorker>,
            StatusCode::CONFLICT,
        ),
        (Arc::new(FailingWorker), StatusCode::BAD_GATEWAY),
    ] {
        let (app, _directory) = application_with(worker).await;
        let response = app
            .oneshot(allocation_request(Some("Bearer valid.token.signature")))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));

        assert_eq!(response.status(), expected);
    }
}

/// A failed allocation used to answer a bare 502 while the reason only reached
/// the daemon log. The recorded worker error now rides in the body so the
/// workflow log can name what broke.
#[tokio::test]
async fn a_failed_allocation_body_names_the_worker_error() {
    let (app, _directory) = application_with(Arc::new(FailingWorker)).await;
    let response = app
        .clone()
        .oneshot(allocation_request(Some("Bearer valid.token.signature")))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let error: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(error["error"], "cannot start Tart VM");
    assert_eq!(error["state"], "failed");
    let id = error["allocation_id"]
        .as_str()
        .unwrap_or_else(|| unreachable!("allocation_id missing: {error}"))
        .to_owned();

    // Polling the failed allocation afterwards reports the same reason.
    let response = app
        .oneshot(probe(
            "GET",
            &format!("/v1/allocations/{id}"),
            Some("Bearer valid.token.signature"),
        ))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let allocation: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(allocation["state"], "failed");
    assert_eq!(allocation["error"], "cannot start Tart VM");
}

#[tokio::test]
async fn allocation_timeout_cancels_and_cleans_the_worker() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let cleaned = Arc::new(AtomicBool::new(false));
    let worker = Arc::new(TimeoutWorker {
        cleaned: cleaned.clone(),
    });
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(ConfigHandle::new(config), store, images, hot, worker);
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
            storage_mb: 40_960,
        })
    );
    assert_eq!(allocation.mode, flanforge_core::AllocationMode::Warm);
}

const CREATE_BODY: &str =
    r#"{"profile":"project","repository":"owner/project","run_id":"42","run_attempt":1}"#;

fn allocation_request(authorization: Option<&str>) -> Request<Body> {
    allocation_request_with_body(authorization, CREATE_BODY)
}

/// One well-formed request per protected method and path, so a refusal can only
/// come from the credential.
fn probe(method: &str, path: &str, authorization: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(AUTHORIZATION, value);
    }
    builder
        .body(Body::from(CREATE_BODY))
        .unwrap_or_else(|error| unreachable!("request: {error}"))
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

/// A peer that completes one request and then goes idle used to hold its
/// connection slot for as long as it liked, because the head deadline disarmed
/// permanently at the first head. `/healthz` needs no credential, so a handful
/// of such sockets could exhaust the bound before any request is authorized.
#[tokio::test]
async fn the_listener_reaps_an_idle_keep_alive_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (application, _directory) = application().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    // One slot, so the last request can only be served once the idle peer's
    // slot is actually released.
    let bounded = super::BoundedListener::new(listener, 1, Duration::from_millis(200));
    tokio::spawn(async move {
        let _ = axum::serve(bounded, application).await;
    });

    let mut idle = tokio::net::TcpStream::connect(address)
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    idle.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap_or_else(|error| unreachable!("write: {error}"));
    let mut answered = vec![0_u8; 1_024];
    let read = tokio::time::timeout(Duration::from_secs(5), idle.read(&mut answered))
        .await
        .unwrap_or_else(|_| unreachable!("the keep-alive request was never answered"))
        .unwrap_or_else(|error| unreachable!("read: {error}"));
    assert!(String::from_utf8_lossy(&answered[..read]).starts_with("HTTP/1.1 200"));

    // Now send nothing further. The next head never arrives, so the connection
    // is reaped rather than held.
    let mut discarded = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), idle.read_to_end(&mut discarded))
        .await
        .unwrap_or_else(|_| unreachable!("idle keep-alive connection was never reaped"));
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
        .unwrap_or_else(|_| unreachable!("the reaped connection slot was never reused"))
        .unwrap_or_else(|error| unreachable!("read: {error}"));
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
}

async fn operator_application() -> (axum::Router, AllocationManager, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        hot,
        Arc::new(ReadyWorker),
    );
    let (users, sessions) = webui_services(&directory).await;
    let state = OperatorState::new(
        manager.clone(),
        config.server.listen,
        OPERATOR_TOKEN.to_owned(),
        users,
        sessions,
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
        Request::get("/v1/operator/hot")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}")),
        Request::post("/v1/operator/hot/ci-project-1-1")
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
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        hot,
        Arc::new(ReadyWorker),
    );
    let (users, sessions) = webui_services(&directory).await;
    let state = OperatorState::new(
        manager,
        config.server.listen,
        OPERATOR_TOKEN.to_owned(),
        users,
        sessions,
    );
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
async fn operator_webui_users_lifecycle() {
    let (app, _manager, _directory) = operator_application().await;

    let create = Request::post("/v1/operator/webui/users")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"username":"jim","password":"a-long-password"}"#,
        ))
        .unwrap_or_else(|error| unreachable!("request: {error}"));
    let response = app
        .clone()
        .oneshot(as_operator(create, "127.0.0.1:51000", listen()))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);

    // A short password and a hostile username are refused at the door.
    for body in [
        r#"{"username":"jim2","password":"short"}"#,
        r#"{"username":"../jim","password":"a-long-password"}"#,
    ] {
        let request = Request::post("/v1/operator/webui/users")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap_or_else(|error| unreachable!("request: {error}"));
        let response = app
            .clone()
            .oneshot(as_operator(request, "127.0.0.1:51000", listen()))
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }

    let reset = Request::post("/v1/operator/webui/users/jim/password")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"another-long-password"}"#))
        .unwrap_or_else(|error| unreachable!("request: {error}"));
    let response = app
        .clone()
        .oneshot(as_operator(reset, "127.0.0.1:51000", listen()))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);

    let delete = Request::delete("/v1/operator/webui/users/jim")
        .body(Body::empty())
        .unwrap_or_else(|error| unreachable!("request: {error}"));
    let response = app
        .clone()
        .oneshot(as_operator(delete, "127.0.0.1:51000", listen()))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);

    let missing = Request::delete("/v1/operator/webui/users/jim")
        .body(Body::empty())
        .unwrap_or_else(|error| unreachable!("request: {error}"));
    let response = app
        .oneshot(as_operator(missing, "127.0.0.1:51000", listen()))
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images,
        hot,
        Arc::new(ReadyWorker),
    );
    // The loopback shape: one listener serving both surfaces.
    let application = super::build_router(
        AppState::new(
            manager.clone(),
            Arc::new(StaticVerifier),
            // Thirty seconds for the same reason as the shared fixture: the
            // wait is wall clock, and ReadyWorker transitions immediately.
            Duration::from_secs(30),
        ),
        4_096,
    )
    .merge(super::build_operator_router(
        {
            let (users, sessions) = webui_services(&directory).await;
            OperatorState::new(
                manager,
                config.server.listen,
                OPERATOR_TOKEN.to_owned(),
                users,
                sessions,
            )
        },
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

/// The browser surface over a fresh database, with one seeded account.
async fn webui_application(
    adjust: impl FnOnce(&mut flanforge_core::Config),
) -> (axum::Router, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    adjust(&mut config);
    let handle = ConfigHandle::new(Arc::new(config));
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("state.db"))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let manager = AllocationManager::new(
        Arc::clone(&handle),
        Arc::new(SqliteAllocationStore::new(connection.clone())),
        Arc::new(SqliteWarmImageStore::new(connection.clone())),
        Arc::new(SqliteHotGuestStore::new(connection.clone())),
        Arc::new(ReadyWorker),
    );
    let users = UserService::new(connection.clone());
    let hash = flanforge_webui_auth::hash_password("a-long-password")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    users
        .create_authdb_user("jim", &hash)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let services = flanforge_handlers::WebuiServices {
        users,
        sessions: SessionService::new(connection.clone()),
        history: flanforge_orm::HistoryService::new(connection),
        manager,
        config: handle,
        event_feed: Arc::new(flanforge_store::BroadcastEventSink::default()),
        oidc: None,
        events: Arc::new(flanforge_store::NullEventSink),
        config_path: {
            let config_path = directory.path().join("config.toml");
            std::fs::write(&config_path, flanforge_config::STARTER_CONFIG)
                .unwrap_or_else(|error| unreachable!("write config: {error}"));
            config_path
        },
        reload_now: Arc::new(tokio::sync::Notify::new()),
        version: "test".to_owned(),
    };
    (super::build_webui_router(services, 65_536, None), directory)
}

/// Logs in and returns the session cookie value pair.
async fn webui_login(app: &axum::Router) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"jim","password":"a-long-password"}"#,
                ))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| unreachable!("login sets the session cookie"));
    cookie.split(';').next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn webui_meta_is_public_and_refusals_are_json_not_redirects() {
    let (app, _directory) = webui_application(|_| {}).await;

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/meta")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);

    // Accounts are a sensitive read: anonymous is 401, JSON, no redirect.
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/users")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .is_none()
    );

    // Unknown /api/v1 paths answer JSON here, never another surface.
    let response = app
        .oneshot(
            Request::get("/api/v1/no-such-thing")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webui_public_read_only_never_opens_the_mutating_tier() {
    let (app, _directory) = webui_application(|config| {
        config.webui.public_read_only = true;
    })
    .await;
    for (method, path) in [
        ("GET", "/api/v1/users"),
        ("POST", "/api/v1/users"),
        ("DELETE", "/api/v1/users/1"),
        ("PUT", "/api/v1/profiles/new-profile"),
        ("DELETE", "/api/v1/profiles/new-profile"),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap_or_else(|error| unreachable!("request: {error}"));
        let response = app
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn webui_login_session_and_csrf_wall() {
    let (app, _directory) = webui_application(|_| {}).await;
    let cookie = webui_login(&app).await;

    // A signed-in GET passes without the CSRF header.
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/users")
                .header(axum::http::header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);

    // A mutation without the custom header is refused even with the session.
    let create_body = r#"{"username":"sam","password":"another-long-password"}"#;
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/users")
                .header(axum::http::header::COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(create_body))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // With the header it lands; a cross-site Origin is still refused.
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/users")
                .header(axum::http::header::COOKIE, &cookie)
                .header("content-type", "application/json")
                .header("x-flanforge-webui", "1")
                .body(Body::from(create_body))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(
            Request::delete("/api/v1/users/1")
                .header(axum::http::header::COOKIE, &cookie)
                .header("x-flanforge-webui", "1")
                .header(axum::http::header::ORIGIN, "https://evil.example")
                .header(axum::http::header::HOST, "ci.example")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Logout clears the session; the next read is anonymous again.
    let response = app
        .clone()
        .oneshot(
            Request::delete("/api/v1/session")
                .header(axum::http::header::COOKIE, &cookie)
                .header("x-flanforge-webui", "1")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .oneshot(
            Request::get("/api/v1/users")
                .header(axum::http::header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webui_serves_a_clear_503_without_a_built_bundle() {
    let (app, _directory) = webui_application(|_| {}).await;
    let response = app
        .oneshot(
            Request::get("/ui/")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    // The embed is whatever webui/dist holds at build time: empty means a
    // clear 503, a built bundle means the SPA shell.
    assert!(matches!(
        response.status(),
        StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
    ));
}

#[tokio::test]
async fn webui_readonly_tier_follows_public_read_only() {
    let (private, _directory) = webui_application(|_| {}).await;
    for path in [
        "/api/v1/status",
        "/api/v1/allocations",
        "/api/v1/allocations/history",
        "/api/v1/hot",
        "/api/v1/warm",
        "/api/v1/sweeps",
        "/api/v1/events",
    ] {
        let response = private
            .clone()
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    let (public, _directory) = webui_application(|config| {
        config.webui.public_read_only = true;
    })
    .await;
    // Configuration stays gated even on a public deployment.
    let response = public
        .clone()
        .oneshot(
            Request::get("/api/v1/config")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for path in ["/api/v1/allocations", "/api/v1/hot", "/api/v1/events"] {
        let response = public
            .clone()
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    // The same path's mutating method stays gated for anonymous callers.
    let response = public
        .oneshot(
            Request::delete("/api/v1/allocations/00000000-0000-4000-8000-000000000000")
                .header("x-flanforge-webui", "1")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webui_streams_sit_in_their_tiers() {
    let (private, _directory) = webui_application(|_| {}).await;
    for path in ["/api/v1/events/stream", "/api/v1/logs/stream"] {
        let response = private
            .clone()
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    let (public, _directory) = webui_application(|config| {
        config.webui.public_read_only = true;
    })
    .await;
    let response = public
        .clone()
        .oneshot(
            Request::get("/api/v1/events/stream")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    // The log remains a sensitive read even on a public deployment.
    let response = public
        .oneshot(
            Request::get("/api/v1/logs/stream")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oidc_endpoints_answer_404_when_not_configured() {
    let (app, _directory) = webui_application(|_| {}).await;
    for path in ["/api/v1/oidc/login", "/api/v1/oidc/callback?code=x&state=y"] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
