use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
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
    ForgejoClient, ForgejoError, JobBinding, JobDiagnosis, RunnerCredentials, RunnerStatus,
    jobs::{JobReject, diagnose_jobs, job_reject, select_job_handle},
    models::ActionRunJob,
    read_secret_file,
};

/// The signed identity every job fixture below is measured against.
fn binding() -> JobBinding<'static> {
    JobBinding {
        label: "macos-allocation",
        job_name: "apple-build",
        run_id: 42,
    }
}

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
        client.job_handle(&repository, &binding()).await,
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

/// RUN-160: Forgejo clamps `limit` to `MaxResponseItems`, so a page shorter
/// than the requested size is not the end of the listing.
#[tokio::test]
async fn a_clamped_page_size_does_not_cut_the_runner_listing_short() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let client = serve_clamped_listing(ClampedListing {
        total: 73,
        clamp: 50,
        matching: Arc::new(vec![73]),
        deleted: deleted.clone(),
    })
    .await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // The only match sits on page 2, which the old page-size test never read.
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

/// RUN-160: a total that is an exact multiple of the page size is where
/// assuming a page size breaks — only the empty page after it ends the listing.
#[tokio::test]
async fn a_listing_that_exactly_fills_its_last_page_still_completes() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let client = serve_clamped_listing(ClampedListing {
        total: 100,
        clamp: 50,
        matching: Arc::new(vec![51, 100]),
        deleted: deleted.clone(),
    })
    .await;
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
        ["51", "100"]
    );
}

/// RUN-160: an unreadable listing is still reported, but losing the matches
/// already found is strictly worse than a partial cleanup.
#[tokio::test]
async fn an_exhausted_page_budget_still_deletes_the_matches_already_found() {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let client = serve_clamped_listing(ClampedListing {
        // An endless listing exhausts the budget whatever the budget is.
        total: usize::MAX,
        clamp: 50,
        matching: Arc::new(vec![7]),
        deleted: deleted.clone(),
    })
    .await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(
        client
            .delete_runners_named(&repository, "flanforged-test")
            .await,
        Err(ForgejoError::Malformed)
    );
    assert_eq!(
        deleted
            .lock()
            .unwrap_or_else(|error| unreachable!("fixture: {error}"))
            .as_slice(),
        ["7"]
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

    assert_eq!(client.job_handle(&repository, &binding()).await, Ok(None));
    assert_eq!(client.job_handle(&empty, &binding()).await, Ok(None));
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
        client.job_handle(&repository, &binding()).await,
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
    assert_eq!(select_job_handle(Vec::new(), &binding()), Ok(None));
    assert_eq!(
        select_job_handle(vec![job(), job()], &binding()),
        Err(ForgejoError::Api)
    );

    // Every structural and policy failure inside the authorized run stays
    // fatal, and each names its own condition rather than sharing one message.
    for (mutate, reject) in [
        (
            (|job: &mut ActionRunJob| job.name = "untrusted-build".into()) as fn(&mut ActionRunJob),
            JobReject::Name,
        ),
        (|job| job.status = "running".into(), JobReject::Status),
        (
            |job| job.runs_on = vec!["macos-other".into()],
            JobReject::Label,
        ),
        (
            |job| job.handle = "$(touch /tmp/host)".into(),
            JobReject::Structure {
                field: "handle",
                code: "handle",
            },
        ),
        (
            |job| job.runs_on = vec!["macos-allocation;touch".into()],
            JobReject::Structure {
                field: "runs_on",
                code: "runner_labels",
            },
        ),
    ] {
        let mut hostile = job();
        mutate(&mut hostile);
        assert_eq!(job_reject(&hostile, &binding()), Some(reject));
        assert_eq!(
            select_job_handle(vec![hostile], &binding()),
            Err(ForgejoError::Api)
        );
    }
}

/// CORE-321: Forgejo publishes the handle as an opaque run-attempt identifier
/// and reserves the right to stop minting UUIDs, so any bounded shell-safe
/// token must bind while an unbounded or hostile one must not.
#[test]
fn an_opaque_job_handle_binds_but_an_unbounded_or_hostile_one_does_not() {
    let with_handle = |handle: &str| {
        let mut job = job();
        job.handle = handle.into();
        job
    };
    let longest = "a".repeat(128);
    for handle in [
        "job_01HZX3QK",
        "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
        "forgejo:run.42-1",
        longest.as_str(),
    ] {
        assert!(with_handle(handle).validate().is_ok(), "{handle}");
        assert_eq!(
            job_reject(&with_handle(handle), &binding()),
            None,
            "{handle}"
        );
        assert_eq!(
            select_job_handle(vec![with_handle(handle)], &binding()),
            Ok(Some(handle.to_owned())),
            "{handle}"
        );
    }

    let too_long = "a".repeat(129);
    for handle in [
        "",
        too_long.as_str(),
        "$(touch /tmp/host)",
        "job'01HZX3QK",
        "job 01HZX3QK",
        "job;touch",
        "job\n01HZX3QK",
    ] {
        assert!(with_handle(handle).validate().is_err(), "{handle:?}");
        assert_eq!(
            job_reject(&with_handle(handle), &binding()),
            Some(JobReject::Structure {
                field: "handle",
                code: "handle",
            }),
            "{handle:?}"
        );
        assert_eq!(
            select_job_handle(vec![with_handle(handle)], &binding()),
            Err(ForgejoError::Api),
            "{handle:?}"
        );
    }
}

/// CORE-321: a response rejected at the JSON boundary logged only a Rust type
/// name, so a changed Forgejo format was undiagnosable from the daemon log.
#[test]
fn a_response_rejected_at_the_json_boundary_names_the_failing_field() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (status, logged) = flanforge_test_support::capture_logs(|| {
        runtime.block_on(async {
            let application = Router::new().route(
                "/api/v1/repos/owner/project/actions/runners/{id}",
                get(|| async { Json(json!({"id": 73, "status": "act ive"})) }),
            );
            let client = serve(application).await;
            let repository = RepositoryName::new("owner/project")
                .unwrap_or_else(|error| unreachable!("fixture: {error}"));
            client.runner_status(&repository, 73).await
        })
    });

    assert_eq!(status, Err(ForgejoError::Malformed));
    assert!(
        logged.contains("field=\"status\"") && logged.contains("code=\"token\""),
        "{logged}"
    );
}

/// ARCH-323: Forgejo keeps a job only when its whole `runs_on` is covered by
/// the query labels, so a one-label search never returns a job carrying an
/// extra entry. The daemon's own check must be that same equality.
#[test]
fn an_extra_runs_on_entry_is_rejected_because_the_search_can_never_return_it() {
    for extra in [
        vec!["macos-allocation".into(), "macos".into()],
        vec!["macos".into(), "macos-allocation".into()],
    ] {
        let mut wider = job();
        wider.runs_on = extra.clone();
        assert_eq!(job_reject(&wider, &binding()), Some(JobReject::Label));
        assert_eq!(
            select_job_handle(vec![wider], &binding()),
            Err(ForgejoError::Api),
            "{extra:?}"
        );
    }

    // The exact single label still binds.
    assert_eq!(
        select_job_handle(vec![job()], &binding()),
        Ok(Some(job().handle))
    );
}

/// ARCH-323: the unlabelled re-read must blame the dependent job only on a
/// signal further polling cannot change, so a slow queue keeps its wait.
#[test]
fn only_a_settled_same_run_job_with_an_unmatchable_runs_on_is_blamed() {
    let mismatched = || {
        let mut job = job();
        job.runs_on = vec!["macos-allocation".into(), "macos".into()];
        job
    };
    assert_eq!(
        diagnose_jobs(vec![mismatched()], &binding()),
        JobDiagnosis::RunsOnMismatch(vec!["macos-allocation".into(), "macos".into()])
    );
    // A job that never carried the label at all is the same misconfiguration.
    let mut foreign_label = job();
    foreign_label.runs_on = vec!["macos-13".into()];
    assert_eq!(
        diagnose_jobs(vec![foreign_label], &binding()),
        JobDiagnosis::RunsOnMismatch(vec!["macos-13".into()])
    );

    // Everything that is merely "not queued yet" keeps waiting.
    let blocked = |mutate: fn(&mut ActionRunJob)| {
        let mut job = mismatched();
        mutate(&mut job);
        diagnose_jobs(vec![job], &binding())
    };
    assert_eq!(diagnose_jobs(Vec::new(), &binding()), JobDiagnosis::Pending);
    assert_eq!(
        diagnose_jobs(vec![job()], &binding()),
        JobDiagnosis::Pending,
        "a correctly queued job is not a fault"
    );
    assert_eq!(
        blocked(|job| job.status = "running".into()),
        JobDiagnosis::Pending
    );
    assert_eq!(
        blocked(|job| job.name = "another-build".into()),
        JobDiagnosis::Pending
    );
    assert_eq!(
        blocked(|job| job.runs_on = vec!["macos-allocation;touch".into()]),
        JobDiagnosis::Pending,
        "an unreadable record decides nothing"
    );

    // A run this allocation did not sign, or a Forgejo that publishes no run,
    // cannot be attributed — and two candidates blame neither.
    assert_eq!(
        blocked(|job| job.run_id = Some(43)),
        JobDiagnosis::Unattributed
    );
    assert_eq!(blocked(|job| job.run_id = None), JobDiagnosis::Unattributed);
    let mut sibling = mismatched();
    sibling.handle = "2f1d3f6c-3a24-4f5f-9a0b-8a52a1b6f0d1".into();
    assert_eq!(
        diagnose_jobs(vec![mismatched(), sibling], &binding()),
        JobDiagnosis::Ambiguous
    );

    // Each answer names itself, so a phase timeout says which one it carried.
    let reasons = [
        JobDiagnosis::Pending,
        JobDiagnosis::Unattributed,
        JobDiagnosis::Ambiguous,
        JobDiagnosis::RunsOnMismatch(Vec::new()),
    ]
    .map(|diagnosis| diagnosis.reason());
    assert_eq!(
        reasons
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        reasons.len()
    );
    assert!(reasons.iter().all(|reason| !reason.is_empty()));
}

/// ARCH-323: the diagnosis must drop the `labels` filter, or Forgejo answers
/// with the same empty list that caused the wait.
#[tokio::test]
async fn the_wait_diagnosis_queries_the_same_endpoint_without_the_label_filter() {
    let application = Router::new().route(
        "/api/v1/repos/owner/project/actions/runners/jobs",
        get(|Query(query): Query<HashMap<String, String>>| async move {
            if query.contains_key("labels") {
                // The server's own filter drops a job carrying an extra label.
                return Json(json!(null));
            }
            Json(json!([{
                "attempt": 1,
                "run_id": 42,
                "handle": "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
                "name": "apple-build",
                "runs_on": ["macos-allocation", "macos"],
                "status": "waiting"
            }]))
        }),
    );
    let client = serve(application).await;
    let repository = RepositoryName::new("owner/project")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(client.job_handle(&repository, &binding()).await, Ok(None));
    assert_eq!(
        client.job_diagnosis(&repository, &binding()).await,
        Ok(JobDiagnosis::RunsOnMismatch(vec![
            "macos-allocation".into(),
            "macos".into()
        ]))
    );
}

/// CORE-322: `attempt` is a per-job counter on a different row from the signed
/// one, so a re-run of either job must not stop the allocation binding.
#[test]
fn a_diverged_attempt_counter_does_not_block_the_authorized_run() {
    let mut reran = job();
    reran.attempt = 7;
    assert_eq!(
        select_job_handle(vec![reran], &binding()),
        Ok(Some("33ba7d51-59c6-44f8-9d2b-1b94f4033973".into()))
    );
}

/// SEC-009: the label is published to the workflow by design, so only the
/// signed run ID decides which listed job may be bound.
#[test]
fn only_the_signed_run_can_bind_and_a_foreign_run_is_not_an_error() {
    let mut foreign = job();
    foreign.run_id = Some(43);
    // Alone, a decoy is simply not our job: keep polling rather than fail the
    // allocation, which repository code could otherwise trigger at will.
    assert_eq!(select_job_handle(vec![foreign], &binding()), Ok(None));

    for order in [false, true] {
        let mut foreign = job();
        foreign.run_id = Some(43);
        foreign.handle = "2f1d3f6c-3a24-4f5f-9a0b-8a52a1b6f0d1".into();
        let mut jobs = vec![foreign, job()];
        if order {
            jobs.reverse();
        }
        assert_eq!(
            select_job_handle(jobs, &binding()),
            Ok(Some("33ba7d51-59c6-44f8-9d2b-1b94f4033973".into())),
            "reversed: {order}"
        );
    }

    // Cardinality applies to the run-matched subset, not the raw listing.
    let mut jobs: Vec<ActionRunJob> = (0..64)
        .map(|_| {
            let mut foreign = job();
            foreign.run_id = Some(43);
            foreign
        })
        .collect();
    jobs.push(job());
    assert_eq!(
        select_job_handle(jobs, &binding()),
        Ok(Some("33ba7d51-59c6-44f8-9d2b-1b94f4033973".into()))
    );

    // Two jobs inside the signed run are genuinely undecidable.
    let mut sibling = job();
    sibling.handle = "2f1d3f6c-3a24-4f5f-9a0b-8a52a1b6f0d1".into();
    assert_eq!(
        select_job_handle(vec![job(), sibling], &binding()),
        Err(ForgejoError::Api)
    );
}

/// Forgejo publishes a job's run only from v16. An absent field is that older
/// shape and must still bind on the label, or every allocation fails; a field
/// that is present but unusable is an anomaly and must not bind at all.
#[test]
fn an_absent_run_id_falls_back_while_an_unusable_one_does_not_bind() {
    let mut older_forge = job();
    older_forge.run_id = None;
    assert_eq!(
        select_job_handle(vec![older_forge], &binding()),
        Ok(Some(job().handle))
    );

    for value in [Some(0), Some(-1)] {
        let mut unusable = job();
        unusable.run_id = value;
        assert_eq!(
            select_job_handle(vec![unusable], &binding()),
            Ok(None),
            "{value:?}"
        );
    }
}

/// A present run id must keep binding even when another listed job lacks one,
/// so one unclassifiable entry cannot downgrade the filter for the rest.
#[test]
fn one_job_without_a_run_id_does_not_drop_the_filter_for_the_others() {
    let mut absent = job();
    absent.run_id = None;
    absent.handle = "2f1d3f6c-3a24-4f5f-9a0b-8a52a1b6f0d1".into();
    let mut foreign = job();
    foreign.run_id = Some(43);
    foreign.handle = "3c2e4a7d-4b35-4a6a-8b1c-9b63b2c7a1e2".into();

    // The genuine job is the only one from the signed run, so it still binds.
    assert_eq!(
        select_job_handle(vec![absent, foreign, job()], &binding()),
        Ok(Some(job().handle))
    );
}

/// ARCH-613: the reject conditions used to share one message carrying only the
/// signed attempt. Each must now name itself, and none may print the
/// per-allocation label or the opaque handle.
#[test]
fn each_reject_condition_names_itself_without_logging_the_label_or_handle() {
    type RejectCase = (fn(&mut ActionRunJob), &'static str);
    let cases: [RejectCase; 6] = [
        (|job| job.run_id = Some(43), "reason=\"foreign_run\""),
        (|job| job.name = "untrusted-build".into(), "reason=\"name\""),
        (|job| job.status = "running".into(), "reason=\"status\""),
        (
            |job| job.runs_on = vec!["macos-other".into()],
            "reason=\"label\"",
        ),
        (
            |job| job.handle = "$(touch /tmp/host)".into(),
            "reason=\"structure\"",
        ),
        (|_| (), "reason=\"ambiguous\""),
    ];
    for (mutate, expected) in cases {
        let mut rejected = job();
        mutate(&mut rejected);
        let jobs = if expected.contains("ambiguous") {
            let mut sibling = job();
            sibling.handle = "2f1d3f6c-3a24-4f5f-9a0b-8a52a1b6f0d1".into();
            vec![rejected, sibling]
        } else {
            vec![rejected]
        };
        let (_, logged) =
            flanforge_test_support::capture_logs(|| select_job_handle(jobs, &binding()));
        assert!(logged.contains(expected), "{expected}: {logged}");
        assert!(
            !logged.contains("macos-allocation"),
            "label logged: {logged}"
        );
        assert!(
            !logged.contains("33ba7d51-59c6-44f8-9d2b-1b94f4033973"),
            "handle logged: {logged}"
        );
    }

    // The success line is the same rule: run and attempt, never the handle.
    let (selected, logged) =
        flanforge_test_support::capture_logs(|| select_job_handle(vec![job()], &binding()));
    assert!(selected.is_ok());
    assert!(
        logged.contains("run_id=42") && logged.contains("attempt=1"),
        "{logged}"
    );
    assert!(
        !logged.contains("33ba7d51-59c6-44f8-9d2b-1b94f4033973"),
        "{logged}"
    );
    assert!(!logged.contains("macos-allocation"), "{logged}");
}

fn job() -> ActionRunJob {
    ActionRunJob {
        attempt: 1,
        run_id: Some(42),
        handle: "33ba7d51-59c6-44f8-9d2b-1b94f4033973".into(),
        name: "apple-build".into(),
        runs_on: vec!["macos-allocation".into()],
        status: "waiting".into(),
    }
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

async fn list_runners_with_foreign_record(
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    if requested_number(&query, "page") > 1 {
        return Json(json!([]));
    }
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

async fn list_runners(
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    assert!(headers.contains_key(AUTHORIZATION));
    if requested_number(&query, "page") > 1 {
        return Json(json!([]));
    }
    Json(json!([{
        "id": 73,
        "name": "flanforged-test",
        "status": "offline"
    }]))
}

/// A fake runner listing that clamps `limit` the way `MaxResponseItems` does,
/// so a client can never infer the page size from what it asked for.
#[derive(Clone, Debug)]
struct ClampedListing {
    total: usize,
    clamp: usize,
    matching: Arc<Vec<usize>>,
    deleted: Arc<Mutex<Vec<String>>>,
}

// Lets the shared deletion recorder read its own state out of the listing.
impl FromRef<ClampedListing> for Arc<Mutex<Vec<String>>> {
    fn from_ref(listing: &ClampedListing) -> Self {
        listing.deleted.clone()
    }
}

async fn serve_clamped_listing(listing: ClampedListing) -> ForgejoClient {
    let application = Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners",
            get(list_clamped_page),
        )
        .route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            delete(record_deletion),
        )
        .with_state(listing);
    serve(application).await
}

async fn list_clamped_page(
    State(listing): State<ClampedListing>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let page = requested_number(&query, "page");
    let requested = requested_number(&query, "limit");
    assert!(page >= 1 && requested >= 1, "pagination must be requested");
    assert_eq!(query.get("visible").map(String::as_str), Some("false"));
    let size = requested.min(listing.clamp);
    let first = (page - 1) * size;
    let records: Vec<Value> = (first..listing.total.min(first + size))
        .map(|index| {
            let id = index + 1;
            let name = if listing.matching.contains(&id) {
                "flanforged-test"
            } else {
                "other-runner"
            };
            json!({"id": id, "name": name, "status": "offline"})
        })
        .collect();
    Json(Value::Array(records))
}

fn requested_number(query: &HashMap<String, String>, key: &str) -> usize {
    query
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
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
        "run_id": 42,
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
