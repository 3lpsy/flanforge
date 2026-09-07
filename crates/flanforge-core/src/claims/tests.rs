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
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 8,
        memory_mb: 12_288,
        storage_mb: 40_960,
        boot_timeout_seconds: 300,
        idle_timeout_seconds: 600,
        job_timeout_seconds: 7_200,
        cleanup_timeout_seconds: 120,
        warm_template: None,
        regeneration_workflow: None,
        hot: None,
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

// A scheduled Forgejo run sends an empty `ref_type`. Nothing reads the claim,
// so rejecting it only ever cost availability (CORE-606).
#[test]
fn empty_ref_type_is_authorized() {
    let mut claims = claims();
    claims.ref_type = String::new();
    assert_eq!(claims.ensure_authorized(&profile(), &request()), Ok(()));
}

#[test]
fn over_long_ref_type_is_still_rejected() {
    let mut claims = claims();
    claims.ref_type = "b".repeat(33);
    assert_eq!(
        claims.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::MalformedClaims)
    );
}

// The full shape a scheduled Forgejo run sends: short ref in all three
// ref-bearing claims, and an empty `ref_type` (CORE-607).
fn scheduled_claims() -> ForgejoClaims {
    let mut claims = claims();
    claims.event_name = "schedule".to_owned();
    claims.git_ref = "main".to_owned();
    claims.ref_type = String::new();
    claims.sub = "repo:owner/halogen:ref:main".to_owned();
    claims.workflow_ref = "owner/halogen/.forgejo/workflows/apple.yml@main".to_owned();
    claims
}

#[test]
fn scheduled_short_ref_is_normalised_and_authorized() {
    let mut profile = profile();
    profile.allowed_events.insert("schedule".to_owned());
    let claims = scheduled_claims().normalized();

    assert_eq!(claims.git_ref, "refs/heads/main");
    assert_eq!(claims.sub, "repo:owner/halogen:ref:refs/heads/main");
    assert_eq!(
        claims.workflow_ref,
        "owner/halogen/.forgejo/workflows/apple.yml@refs/heads/main"
    );
    assert_eq!(claims.ensure_authorized(&profile, &request()), Ok(()));
}

#[test]
fn normalisation_does_not_touch_a_full_ref() {
    assert_eq!(claims().normalized(), claims());
}

#[test]
fn a_short_ref_outside_policy_is_still_refused() {
    let mut profile = profile();
    profile.allowed_events.insert("schedule".to_owned());
    let mut claims = scheduled_claims();
    claims.git_ref = "attacker".to_owned();
    claims.sub = "repo:owner/halogen:ref:attacker".to_owned();
    claims.workflow_ref = "owner/halogen/.forgejo/workflows/apple.yml@attacker".to_owned();
    assert_eq!(
        claims.normalized().ensure_authorized(&profile, &request()),
        Err(AuthorizationError::GitRef)
    );
}

// Only the claims that actually carry the short ref are rewritten, so a token
// whose subject disagrees fails the subject check instead of being repaired.
#[test]
fn normalisation_does_not_repair_a_disagreeing_subject() {
    let mut profile = profile();
    profile.allowed_events.insert("schedule".to_owned());
    let mut claims = scheduled_claims();
    claims.sub = "repo:owner/halogen:ref:refs/heads/other".to_owned();
    assert_eq!(
        claims.normalized().ensure_authorized(&profile, &request()),
        Err(AuthorizationError::Subject)
    );
}

#[test]
fn glob_patterns_authorize_workflows_and_refs() {
    let mut profile = profile();
    profile.allowed_workflows = BTreeSet::from(["*.yml".to_owned()]);
    profile.allowed_refs = BTreeSet::from(["refs/heads/*".to_owned()]);
    assert_eq!(claims().ensure_authorized(&profile, &request()), Ok(()));
}

#[test]
fn a_glob_authorizes_nothing_outside_its_pattern() {
    let mut refs_elsewhere = profile();
    refs_elsewhere.allowed_refs = BTreeSet::from(["refs/tags/v*".to_owned()]);
    assert_eq!(
        claims().ensure_authorized(&refs_elsewhere, &request()),
        Err(AuthorizationError::GitRef)
    );

    let mut other_workflows = profile();
    other_workflows.allowed_workflows = BTreeSet::from(["release-*.yml".to_owned()]);
    assert_eq!(
        claims().ensure_authorized(&other_workflows, &request()),
        Err(AuthorizationError::Workflow)
    );
}

// Several profiles may name one repository (ARCH-122). Policy is read from the
// profile the request selects, so a permissive sibling widens nothing: it
// neither lends its workflow allowlist nor relaxes a protected-ref rule.
#[test]
fn a_sibling_profile_for_the_same_repository_widens_nothing() {
    let mut release = profile();
    release.runner_label =
        RunnerLabel::new("macos-tart-release").unwrap_or_else(|error| unreachable!("{error}"));
    release.allowed_workflows = BTreeSet::from(["release.yml".to_owned()]);
    release.require_protected_ref = true;

    let mut check = profile();
    check.runner_label =
        RunnerLabel::new("macos-tart-check").unwrap_or_else(|error| unreachable!("{error}"));
    check.allowed_workflows = BTreeSet::from(["pull-request.yml".to_owned()]);
    check.require_protected_ref = false;
    assert_eq!(release.repository, check.repository);

    let mut unprotected = claims();
    unprotected.ref_protected = "false".to_owned();
    unprotected.workflow_ref =
        "owner/halogen/.forgejo/workflows/pull-request.yml@refs/heads/main".to_owned();

    // The sibling authorizes exactly its own workflow.
    assert_eq!(
        unprotected.ensure_authorized(&check, &request()),
        Ok(()),
        "the sibling refused its own workflow"
    );
    assert_eq!(
        unprotected.ensure_authorized(&release, &request()),
        Err(AuthorizationError::Workflow)
    );

    // And its relaxed protected-ref rule stays its own.
    let mut unprotected_release = unprotected;
    unprotected_release.workflow_ref =
        "owner/halogen/.forgejo/workflows/release.yml@refs/heads/main".to_owned();
    assert_eq!(
        unprotected_release.ensure_authorized(&release, &request()),
        Err(AuthorizationError::ProtectedRef)
    );
}

// Events are matched literally, so a pattern that would otherwise cover the
// claim authorizes nothing — including the wildcard that reaches
// `pull_request_target`.
#[test]
fn events_are_never_matched_as_patterns() {
    for pattern in ["workflow_*", "*", "?orkflow_dispatch"] {
        let mut profile = profile();
        profile.allowed_events = BTreeSet::from([pattern.to_owned()]);
        assert_eq!(
            claims().ensure_authorized(&profile, &request()),
            Err(AuthorizationError::Event),
            "pattern {pattern} authorized an event"
        );
    }
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

    // The anchor of the job binding: without this the selector would compare a
    // Forgejo run against a number the caller chose, binding nothing (SEC-009).
    let mut wrong_run = request();
    wrong_run.run_id = 43;
    assert_eq!(
        claims().ensure_authorized(&profile(), &wrong_run),
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

// `sub` and `workflow_ref` embed the ref, so all three move together.
fn claims_for_ref(git_ref: &str) -> ForgejoClaims {
    let mut claims = claims();
    claims.git_ref = git_ref.to_owned();
    claims.sub = format!("repo:owner/halogen:ref:{git_ref}");
    claims.workflow_ref = format!("owner/halogen/.forgejo/workflows/apple.yml@{git_ref}");
    claims
}

// The one-for-one migration off `allowed_ref_prefixes`: the prefix
// `refs/tags/v` is spelled `refs/tags/v*` and authorizes the same tags
// (CORE-609).
#[test]
fn a_ref_glob_replaces_the_retired_prefix_form() {
    let mut profile = profile();
    profile.allowed_refs = BTreeSet::from(["refs/tags/v*".to_owned()]);
    assert_eq!(
        claims_for_ref("refs/tags/v1.2.3").ensure_authorized(&profile, &request()),
        Ok(())
    );
    assert_eq!(
        claims_for_ref("refs/tags/nightly").ensure_authorized(&profile, &request()),
        Err(AuthorizationError::GitRef)
    );
}

// The boundary the retired prefix form could not express: `refs/heads/release`
// was the only spelling it accepted, and that also matched `release-x`. A glob
// anchors on the separator (SEC-100).
#[test]
fn a_ref_glob_anchors_a_namespace_on_its_separator() {
    let mut profile = profile();
    profile.allowed_refs = BTreeSet::from(["refs/heads/release/*".to_owned()]);
    assert_eq!(
        claims_for_ref("refs/heads/release/1.2").ensure_authorized(&profile, &request()),
        Ok(())
    );
    assert_eq!(
        claims_for_ref("refs/heads/release-x").ensure_authorized(&profile, &request()),
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

// The repository check has three parts and only the request half was covered.
// A profile naming another repository must not authorize this token, and the
// owner claim has to agree with the repository it is the owner of.
#[test]
fn a_repository_claim_must_match_the_profile_and_the_owner() {
    let mut elsewhere = profile();
    elsewhere.repository =
        RepositoryName::new("owner/liftfg").unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        claims().ensure_authorized(&elsewhere, &request()),
        Err(AuthorizationError::Repository)
    );

    let mut wrong_owner = claims();
    wrong_owner.repository_owner = "attacker".to_owned();
    assert_eq!(
        wrong_owner.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Repository)
    );
}

// The workflow claim carries the ref it ran on, and it has to be the ref the
// token claims. Without that agreement a build on any branch could present a
// workflow pinned to an allowed one, and pass the ref policy on its name.
#[test]
fn a_workflow_ref_from_another_branch_is_refused() {
    let mut claims = claims();
    claims.workflow_ref = "owner/halogen/.forgejo/workflows/apple.yml@refs/heads/other".to_owned();
    assert_eq!(
        claims.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Workflow)
    );
}

// A pull request signs a subject that names no ref, so the two subject forms
// are not interchangeable: each event gets the one it is entitled to.
#[test]
fn a_pull_request_subject_uses_its_own_form() {
    let mut pull_request_profile = profile();
    pull_request_profile.allowed_events = BTreeSet::from(["pull_request".to_owned()]);

    let mut pull_request = claims();
    pull_request.event_name = "pull_request".to_owned();
    pull_request.sub = "repo:owner/halogen:pull_request".to_owned();
    assert_eq!(
        pull_request.ensure_authorized(&pull_request_profile, &request()),
        Ok(())
    );

    // The ref form does not authorize a pull request.
    let mut ref_subject = pull_request.clone();
    ref_subject.sub = "repo:owner/halogen:ref:refs/heads/main".to_owned();
    assert_eq!(
        ref_subject.ensure_authorized(&pull_request_profile, &request()),
        Err(AuthorizationError::Subject)
    );

    // Nor does the pull-request form authorize anything else. The base profile
    // allows `workflow_dispatch`, so only the subject can refuse this.
    let mut dispatch = pull_request;
    dispatch.event_name = "workflow_dispatch".to_owned();
    assert_eq!(
        dispatch.ensure_authorized(&profile(), &request()),
        Err(AuthorizationError::Subject)
    );
}
