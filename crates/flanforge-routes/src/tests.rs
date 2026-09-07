use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
use flanforge_auth::{AuthError, ProviderFailure, TokenRejection};
use flanforge_core::{Allocation, AllocationMode, AllocationState, RunnerLabel, VmName};
use flanforge_manager::{ManagerError, WorkerError};
use flanforge_test_support::{self as test_support, capture_logs};

use super::error::{ApiError, WorkerFailure};

/// ARCH-613: a refusal used to log one `debug!` with no variant, which the
/// daemon's `info` filter dropped entirely. Every reason must now reach an
/// operator, discriminated, without the response body learning anything.
#[test]
fn every_authentication_refusal_names_its_reason_at_a_level_the_daemon_runs() {
    for (error, expected) in [
        (
            AuthError::MissingCredentials,
            ["reason=\"missing_credentials\""].as_slice(),
        ),
        (
            AuthError::InvalidToken(TokenRejection::Signature),
            ["reason=\"signature\"", "field=\"-\"", "detail=\"-\""].as_slice(),
        ),
        (
            AuthError::InvalidToken(TokenRejection::MissingClaim("sub")),
            ["reason=\"missing_claim\"", "field=\"sub\""].as_slice(),
        ),
        (
            AuthError::InvalidToken(TokenRejection::ClaimShape {
                field: "ref_type",
                code: "length",
            }),
            [
                "reason=\"claim_shape\"",
                "field=\"ref_type\"",
                "detail=\"length\"",
            ]
            .as_slice(),
        ),
        (
            AuthError::Unavailable(ProviderFailure::EmptyKeySet),
            ["reason=\"empty_key_set\""].as_slice(),
        ),
    ] {
        let (_, logged) = capture_logs(|| ApiError::from(error));
        for field in expected {
            assert!(
                logged.contains(field),
                "{error:?} missing {field}: {logged}"
            );
        }
    }
}

#[test]
fn persistence_failure_is_an_internal_api_error() {
    let error = ManagerError::Store(std::io::Error::other("fixture").into());
    let response = ApiError::from(error).into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

fn failed_allocation(reason: &str) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-a").unwrap_or_else(|error| unreachable!("{error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    for state in [AllocationState::Cleaning, AllocationState::Failed] {
        allocation
            .transition(state)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    allocation.set_error(reason);
    allocation
}

/// The recorded worker error must reach the authenticated caller: a bare 502
/// left the workflow log with nothing to act on.
#[tokio::test]
async fn a_failed_allocation_body_carries_its_recorded_reason() {
    let allocation = failed_allocation("cannot install guest runner");
    let id = allocation.id;
    let error = ApiError::Worker(WorkerFailure::from_allocation(&allocation));
    let response = error.into_response();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let bytes = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|error| unreachable!("json: {error}"));
    assert_eq!(body["error"], "cannot install guest runner");
    assert_eq!(body["allocation_id"], id.to_string());
    assert_eq!(body["state"], "failed");
}

/// A worker failure with nothing recorded keeps the generic body: no null
/// fields, no invented detail.
#[tokio::test]
async fn a_worker_failure_without_a_reason_keeps_the_generic_body() {
    let response = ApiError::Worker(WorkerFailure::default()).into_response();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let bytes = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        r#"{"error":"FlanForge allocation failed"}"#
    );
}

/// A `WorkerError` surfaced through the manager keeps its message on the way
/// to the body, on top of being logged.
#[tokio::test]
async fn a_manager_worker_error_body_names_the_worker_message() {
    let (error, _) = capture_logs(|| {
        ApiError::from(ManagerError::Worker(WorkerError::new(
            "cannot install guest runner",
        )))
    });
    let response = error.into_response();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let bytes = to_bytes(response.into_body(), 16_384)
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"));
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        r#"{"error":"cannot install guest runner"}"#
    );
}
