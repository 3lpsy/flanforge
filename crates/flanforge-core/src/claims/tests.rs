use std::collections::BTreeSet;

use super::*;
use crate::{
    AllocationRequest, NetworkMode, Profile, ProfileName, RepositoryName, RunnerLabel, VmName,
};

fn profile() -> Profile {
    Profile {
        repository: RepositoryName::new("owner/halogen")
            .unwrap_or_else(|error| unreachable!("{error}")),
        template: VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("{error}")),
        runner_label: RunnerLabel::new("macos-tart-halogen")
            .unwrap_or_else(|error| unreachable!("{error}")),
        job_name: "apple-build".into(),
        allowed_workflows: BTreeSet::from(["apple.yml".to_owned()]),
        allowed_events: BTreeSet::from(["workflow_dispatch".to_owned()]),
        allowed_refs: BTreeSet::from(["refs/heads/main".to_owned()]),
        allowed_ref_prefixes: BTreeSet::new(),
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 8,
        memory_mb: 12_288,
        boot_timeout_seconds: 300,
        idle_timeout_seconds: 600,
        job_timeout_seconds: 7_200,
        cleanup_timeout_seconds: 120,
        warm_template: None,
        regeneration_workflow: None,
        reap: true,
    }
}

fn request() -> AllocationRequest {
    AllocationRequest {
        profile: ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}")),
        repository: RepositoryName::new("owner/halogen")
            .unwrap_or_else(|error| unreachable!("{error}")),
        run_id: 42,
        run_attempt: 1,
    }
}

fn claims() -> ForgejoClaims {
    ForgejoClaims {
        actor: "builder".to_owned(),
        aud: "flanforged".to_owned(),
        event_name: "workflow_dispatch".to_owned(),
        exp: 2_000_000_000,
        iat: 1_999_999_000,
        iss: "https://forgejo.example/api/actions".to_owned(),
        nbf: 1_999_999_000,
        git_ref: "refs/heads/main".to_owned(),
        ref_protected: "true".to_owned(),
        ref_type: "branch".to_owned(),
        repository: "owner/halogen".to_owned(),
        repository_owner: "owner".to_owned(),
        run_attempt: "1".to_owned(),
        run_id: "42".to_owned(),
        run_number: "7".to_owned(),
        sha: "a".repeat(40),
        sub: "repo:owner/halogen:ref:refs/heads/main".to_owned(),
        workflow: "apple.yml".to_owned(),
        workflow_ref: "owner/halogen/.forgejo/workflows/apple.yml@refs/heads/main".to_owned(),
    }
}

#[test]
fn matching_claims_are_authorized() {
    assert_eq!(claims().ensure_authorized(&profile(), &request()), Ok(()));
}

#[test]
fn workflow_display_name_does_not_replace_the_file_identity() {
    let mut claims = claims();
    claims.workflow = "Apple Build and Test".into();
    assert_eq!(claims.ensure_authorized(&profile(), &request()), Ok(()));

    claims.workflow_ref = "owner/project/.forgejo/workflows/untrusted.yml@refs/heads/main".into();
    assert_eq!(
        claims.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Workflow)
    );
}

#[test]
fn request_cannot_swap_repository_or_attempt() {
    let mut wrong_repository = request();
    wrong_repository.repository =
        RepositoryName::new("owner/liftfg").unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        claims().ensure_authorized(&profile(), &wrong_repository),
        Err(AuthorizationError::Repository)
    );

    let mut wrong_attempt = request();
    wrong_attempt.run_attempt = 2;
    assert_eq!(
        claims().ensure_authorized(&profile(), &wrong_attempt),
        Err(AuthorizationError::RunIdentity)
    );
}

#[test]
fn unprotected_or_wrong_workflow_is_rejected() {
    let mut unprotected = claims();
    unprotected.ref_protected = "false".to_owned();
    assert_eq!(
        unprotected.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::ProtectedRef)
    );

    let mut wrong_workflow = claims();
    wrong_workflow.workflow_ref =
        "owner/project/.forgejo/workflows/attacker.yml@refs/heads/main".to_owned();
    assert_eq!(
        wrong_workflow.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Workflow)
    );
}

#[test]
fn inconsistent_subject_is_rejected() {
    let mut claims = claims();
    claims.sub = "repo:owner/halogen:ref:refs/heads/other".to_owned();
    assert_eq!(
        claims.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Subject)
    );
}

#[test]
fn exact_ref_does_not_authorize_a_sibling_branch() {
    let mut claims = claims();
    claims.git_ref = "refs/heads/main-attacker".into();
    claims.workflow_ref =
        "owner/halogen/.forgejo/workflows/apple.yml@refs/heads/main-attacker".into();
    claims.sub = "repo:owner/halogen:ref:refs/heads/main-attacker".into();
    assert_eq!(
        claims.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::GitRef)
    );
}

#[test]
fn malformed_claim_fields_are_rejected_structurally() {
    let mut cases = Vec::new();

    let mut invalid = claims();
    invalid.actor = "builder\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.aud = "flanforged\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.event_name = "workflow_dispatch;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.iss = "https://forgejo.example\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.git_ref = "main;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.ref_protected = "yes".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.run_number = "7;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.ref_type = "branch;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.repository = "owner/halogen;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.repository_owner = "owner;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.run_attempt = "1;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.run_id = "42;touch".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.sha = "g".repeat(40);
    cases.push(invalid);

    let mut invalid = claims();
    invalid.sub = "repo:owner/halogen\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.workflow = "apple.yml\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.workflow_ref = "owner/halogen/.forgejo/workflows/apple.yml\nforged".into();
    cases.push(invalid);

    let mut invalid = claims();
    invalid.exp = 0;
    cases.push(invalid);

    let mut invalid = claims();
    invalid.iat = 0;
    cases.push(invalid);

    let mut invalid = claims();
    invalid.nbf = 0;
    cases.push(invalid);

    for invalid in cases {
        assert_eq!(
            invalid.ensure_authorized(&profile(), &request()),
            Err(AuthorizationError::MalformedClaims)
        );
    }
}
