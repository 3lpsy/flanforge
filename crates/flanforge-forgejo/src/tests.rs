use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{
        HeaderMap, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    routing::{delete, get},
};
use flanforge_core::{ForgejoConfig, RepositoryName};
use serde_json::{Value, json};
use url::Url;
use validator::Validate;

use super::{
    ForgejoClient, ForgejoError, RunnerCredentials, RunnerStatus, jobs::select_job_handle,
    models::ActionRunJob, read_secret_file,
};

#[tokio::test]
async fn reads_a_private_token_file() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("token");
    tokio::fs::write(&path, "12345678901234567890\n")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_eq!(
        read_secret_file(&path).await.as_deref(),
        Ok("12345678901234567890")
    );
}

#[tokio::test]
async fn rejects_a_group_readable_token_file() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("token");
    tokio::fs::write(&path, "12345678901234567890")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_eq!(read_secret_file(&path).await, Err(ForgejoError::Credential));
}

#[tokio::test]
async fn uses_repository_scoped_ephemeral_runner_endpoints() {
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners",
            get(list_runners).post(create_runner),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            get(list_jobs),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            get(get_runner).delete(delete_runner),
        );
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let credentials = client
        .create_runner(&repository, "flanforged-test")
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(credentials.id, 73);
    assert_eq!(
        client.runner_status(&repository, credentials.id).await,
        Ok(RunnerStatus::Active)
    );
    assert_eq!(
        client.delete_runner(&repository, credentials.id).await,
        Ok(())
    );
    assert_eq!(
        client
            .delete_runners_named(&repository, "flanforged-test")
            .await,
        Ok(())
    );
    assert_eq!(
        client
            .job_handle(&repository, "macos-allocation", "apple-build", 1)
            .await,
        Ok(Some("33ba7d51-59c6-44f8-9d2b-1b94f4033973".into()))
    );
}

#[tokio::test]
async fn a_foreign_runner_record_does_not_block_name_based_cleanup() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners",
            get(list_runners_with_foreign_record),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            delete(record_deletion),
        )
        .with_state(deleted.clone());
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(
        client
            .delete_runners_named(&repository, "flanforged-test")
            .await,
        Ok(())
    );
    assert_eq!(
        deleted
            .lock()
            .unwrap_or_else(|error| unreachable!("fixture: {error}"))
            .as_slice(),
        ["73"]
    );
}

#[tokio::test]
async fn an_absent_job_listing_means_keep_waiting_rather_than_a_failure() {
    // Forgejo answers a search with no matches as `null`, and an empty body is
    // the same absence; both must poll again instead of failing the allocation.
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            get(|| async { ([(CONTENT_TYPE, "application/json")], "null") }),
        )
        .route(
            "/api/v1/repos/owner/empty/actions/runners/jobs",
            get(|| async { ([(CONTENT_TYPE, "application/json")], "") }),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners",
            get(|| async { ([(CONTENT_TYPE, "application/json")], "null") }),
        );
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let empty =
        RepositoryName::new("owner/empty").unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(
        client
            .job_handle(&repository, "macos-allocation", "apple-build", 1)
            .await,
        Ok(None)
    );
    assert_eq!(
        client
            .job_handle(&empty, "macos-allocation", "apple-build", 1)
            .await,
        Ok(None)
    );
    // Reconciliation reads the same shape, so an empty runner list is not an error.
    assert_eq!(
        client
            .delete_runners_named(&repository, "flanforged-test")
            .await,
        Ok(())
    );
}

#[tokio::test]
async fn server_errors_and_rate_limits_are_transient_but_client_errors_are_not() {
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            get(|| async { StatusCode::TOO_MANY_REQUESTS }),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners",
            get(|| async { StatusCode::FORBIDDEN }),
        );
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(
        client.runner_status(&repository, 73).await,
        Err(ForgejoError::Transient(StatusCode::SERVICE_UNAVAILABLE))
    );
    assert_eq!(
        client
            .job_handle(&repository, "macos-allocation", "apple-build", 1)
            .await,
        Err(ForgejoError::Transient(StatusCode::TOO_MANY_REQUESTS))
    );
    assert_eq!(
        client
            .delete_runners_named(&repository, "flanforged-test")
            .await,
        Err(ForgejoError::Api)
    );
}

#[tokio::test]
async fn a_deleted_runner_registration_reads_as_absent_rather_than_a_rejection() {
    let application = Router::new().route(
        "/api/v1/repos/owner/project/actions/runners/{id}",
        get(|| async { StatusCode::NOT_FOUND }).delete(|| async { StatusCode::NOT_FOUND }),
    );
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(
        client.runner_status(&repository, 73).await,
        Err(ForgejoError::Absent)
    );
    // An absent resource never comes back, so it joins the non-retryable set.
    assert!(!ForgejoError::Absent.is_retryable());
    assert!(!ForgejoError::Api.is_retryable());
    assert!(ForgejoError::Unavailable.is_retryable());
    assert!(ForgejoError::Transient(StatusCode::BAD_GATEWAY).is_retryable());
    // Four endpoints share the check, so the message names no one resource.
    assert_eq!(
        ForgejoError::Absent.to_string(),
        "Forgejo has no such resource"
    );
    // Cleanup still reads an already-absent runner as success.
    assert_eq!(client.delete_runner(&repository, 73).await, Ok(()));
}

#[tokio::test]
async fn an_unreadable_response_is_malformed_rather_than_a_rejection() {
    // A renamed status or a changed payload is Forgejo answering, not Forgejo
    // refusing: the runner it describes may still be running its job.
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            get(|| async { Json(json!({"id": 73, "status": "occupied"})) }),
        )
        .route(
            "/api/v1/repos/owner/empty/actions/runners/{id}",
            get(|| async { ([(CONTENT_TYPE, "application/json")], "{") }),
        )
        .route(
            "/api/v1/repos/owner/blank/actions/runners/{id}",
            get(|| async { Json(json!({"id": 73, "status": ""})) }),
        );
    let client = serve(application).await;

    for owner in ["owner/project", "owner/empty", "owner/blank"] {
        let repository =
            RepositoryName::new(owner).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        assert_eq!(
            client.runner_status(&repository, 73).await,
            Err(ForgejoError::Malformed),
            "{owner}"
        );
    }
    // Unreadable is neither retryable nor a rejection, so an observer holds its
    // idle window open instead of giving up on the allocation.
    assert!(!ForgejoError::Malformed.is_retryable());
    assert!(!ForgejoError::Malformed.is_rejection());
    assert!(ForgejoError::Api.is_rejection());
    assert!(ForgejoError::Absent.is_rejection());
    assert!(ForgejoError::Configuration.is_rejection());
    assert!(!ForgejoError::Unavailable.is_rejection());
    assert!(!ForgejoError::Transient(StatusCode::BAD_GATEWAY).is_rejection());
}

#[test]
fn job_handle_selection_fails_closed_on_ambiguity_or_mismatch() {
    let job = || ActionRunJob {
        attempt: 1,
        handle: "33ba7d51-59c6-44f8-9d2b-1b94f4033973".into(),
        name: "apple-build".into(),
        runs_on: vec!["macos-allocation".into()],
        status: "waiting".into(),
    };
    assert_eq!(
        select_job_handle(Vec::new(), "macos-allocation", "apple-build", 1),
        Ok(None)
    );
    assert_eq!(
        select_job_handle(vec![job(), job()], "macos-allocation", "apple-build", 1),
        Err(ForgejoError::Api)
    );
    let mut wrong = job();
    wrong.name = "untrusted-build".into();
    assert_eq!(
        select_job_handle(vec![wrong], "macos-allocation", "apple-build", 1),
        Err(ForgejoError::Api)
    );

    let mut hostile = job();
    hostile.handle = "$(touch /tmp/host)".into();
    assert_eq!(
        select_job_handle(vec![hostile], "macos-allocation", "apple-build", 1),
        Err(ForgejoError::Api)
    );

    let mut hostile = job();
    hostile.runs_on = vec!["macos-allocation;touch".into()];
    assert_eq!(
        select_job_handle(vec![hostile], "macos-allocation", "apple-build", 1),
        Err(ForgejoError::Api)
    );
}

#[test]
fn runner_credentials_are_structurally_validated() {
    let credentials = || RunnerCredentials {
        id: 73,
        uuid: "392c9434-6bb9-454b-b9ff-646875cf6691".into(),
        token: "09d130cf90f9d757d83e5cc5a5338c470f04b71c".into(),
    };
    assert!(credentials().validate().is_ok());

    let mut invalid = credentials();
    invalid.id = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = credentials();
    invalid.uuid = "$(touch)".into();
    assert!(invalid.validate().is_err());

    let mut invalid = credentials();
    invalid.token = "$(touch)".into();
    assert!(invalid.validate().is_err());
}

async fn serve(application: Router) -> ForgejoClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });
    let config = Arc::new(ForgejoConfig {
        api_url: Url::parse(&format!("http://{address}/api/v1/"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        api_token_file: "/unused".into(),
        http_timeout_seconds: 5,
    });
    ForgejoClient::new(config, "12345678901234567890".into())
        .unwrap_or_else(|error| unreachable!("client: {error}"))
}

async fn list_runners_with_foreign_record() -> Json<Value> {
    Json(json!([
        {"id": 91, "name": "a".repeat(200), "status": "idle"},
        {"id": 73, "name": "flanforged-test", "status": "offline"}
    ]))
}

async fn record_deletion(
    State(deleted): State<Arc<Mutex<Vec<String>>>>,
    Path(id): Path<String>,
) -> StatusCode {
    if let Ok(mut deleted) = deleted.lock() {
        deleted.push(id);
    }
    StatusCode::NO_CONTENT
}

async fn create_runner(headers: HeaderMap, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer 12345678901234567890")
    );
    assert_eq!(body["ephemeral"], true);
    assert_eq!(body["name"], "flanforged-test");
    (
        StatusCode::CREATED,
        Json(json!({
            "id": 73,
            "uuid": "392c9434-6bb9-454b-b9ff-646875cf6691",
            "token": "09d130cf90f9d757d83e5cc5a5338c470f04b71c"
        })),
    )
}

async fn get_runner(headers: HeaderMap) -> Json<Value> {
    assert!(headers.contains_key(AUTHORIZATION));
    Json(json!({"id": 73, "status": "active"}))
}

async fn list_runners(headers: HeaderMap) -> Json<Value> {
    assert!(headers.contains_key(AUTHORIZATION));
    Json(json!([{
        "id": 73,
        "name": "flanforged-test",
        "status": "offline"
    }]))
}

async fn list_jobs(
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    assert!(headers.contains_key(AUTHORIZATION));
    assert_eq!(
        query.get("labels").map(String::as_str),
        Some("macos-allocation")
    );
    Json(json!([{
        "attempt": 1,
        "handle": "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
        "name": "apple-build",
        "runs_on": ["macos-allocation"],
        "status": "waiting"
    }]))
}

async fn delete_runner(headers: HeaderMap) -> StatusCode {
    assert!(headers.contains_key(AUTHORIZATION));
    StatusCode::NO_CONTENT
}
