use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use flanforge_cli::{NetworkArgument, ProfileCommand, ProfileCreateArgs, ProfileSetArgs};
use flanforge_config::STARTER_CONFIG;
use flanforge_core::ProfileName;
use toml::Value;

use super::parse::{parse_string_list, parse_value};
use super::run::run_profile_command;

fn profile_name(name: &str) -> ProfileName {
    ProfileName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn write_config(directory: &Path, text: &str) -> PathBuf {
    let path = directory.join("config.toml");
    std::fs::write(&path, text).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    path
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| unreachable!("read: {error}"))
}

#[test]
fn profile_set_values_are_typed_and_cannot_inject_toml() {
    assert_eq!(
        parse_value("cpu_count", "8")
            .ok()
            .and_then(|value| value.as_integer()),
        Some(8)
    );
    assert!(parse_value("cpu_count", "8\nevil = true").is_err());
    assert!(parse_string_list("[\"main\"]\nevil = true").is_err());
    assert_eq!(
        parse_string_list("push, workflow_dispatch")
            .map(|value| value.to_string())
            .ok()
            .as_deref(),
        Some("[\"push\", \"workflow_dispatch\"]")
    );
}

#[test]
fn all_profile_keys_are_explicitly_allowlisted() {
    let expected = BTreeSet::from([
        "allowed_events",
        "allowed_refs",
        "allowed_workflows",
        "boot_timeout_seconds",
        "cleanup_timeout_seconds",
        "cpu_count",
        "idle_timeout_seconds",
        "job_name",
        "job_timeout_seconds",
        "memory_mb",
        "storage_mb",
        "network",
        "repository",
        "require_protected_ref",
        "runner_label",
        "template",
    ]);
    for key in expected {
        let sample = match key {
            "require_protected_ref" => "true",
            "cpu_count"
            | "memory_mb"
            | "storage_mb"
            | "boot_timeout_seconds"
            | "cleanup_timeout_seconds"
            | "idle_timeout_seconds"
            | "job_timeout_seconds" => "1",
            _ => "value",
        };
        assert!(parse_value(key, sample).is_ok(), "missing key {key}");
    }
    assert!(parse_value("command", "touch /tmp/host").is_err());

    // A removed key is refused with its migration, not as merely unknown.
    let retired = parse_value("allowed_ref_prefixes", "refs/tags/v")
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(retired.contains("has been removed"), "{retired}");
    assert!(retired.contains("refs/tags/v*"), "{retired}");
}

#[test]
fn profile_create_omits_undeclared_warm_fields_from_the_document() {
    let mut profile = flanforge_test_support::profile();
    profile.warm_template = None;
    profile.regeneration_workflow = None;
    let document = Value::try_from(profile).unwrap_or_else(|error| unreachable!("{error}"));
    let table = document
        .as_table()
        .unwrap_or_else(|| unreachable!("profile table"));
    assert!(!table.contains_key("warm_template"));
    assert!(!table.contains_key("regeneration_workflow"));
    assert_eq!(table.get("reap"), Some(&Value::Boolean(true)));

    let restored = table
        .clone()
        .try_into::<flanforge_core::Profile>()
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(restored.warm_template, None);
    assert!(restored.reap);
}

#[test]
fn warm_status_reports_the_declared_image_or_says_it_is_absent() {
    let profile = Value::try_from(flanforge_test_support::profile())
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(super::run::warm_status(&profile).contains("not declared"));

    let mut warm = flanforge_test_support::profile();
    warm.warm_template = flanforge_core::VmName::new("project-warm").ok();
    warm.regeneration_workflow = Some("warm.yml".to_owned());
    let profile = Value::try_from(warm).unwrap_or_else(|error| unreachable!("{error}"));
    let status = super::run::warm_status(&profile);
    assert!(status.contains("declared as project-warm"));
    assert!(status.contains("produced only by warm.yml"));
}

fn create_arguments(name: &str) -> ProfileCreateArgs {
    let profile = flanforge_test_support::profile();
    ProfileCreateArgs {
        profile: profile_name(name),
        repository: profile.repository,
        template: profile.template,
        runner_label: profile.runner_label,
        job_name: profile.job_name,
        allowed_workflows: profile.allowed_workflows.into_iter().collect(),
        allowed_events: profile.allowed_events.into_iter().collect(),
        allowed_refs: profile.allowed_refs.into_iter().collect(),
        require_protected_ref: profile.require_protected_ref,
        network: NetworkArgument::Softnet,
        cpu_count: profile.cpu_count,
        memory_mb: profile.memory_mb,
        storage_mb: profile.storage_mb,
        boot_timeout_seconds: profile.boot_timeout_seconds,
        idle_timeout_seconds: profile.idle_timeout_seconds,
        job_timeout_seconds: profile.job_timeout_seconds,
        cleanup_timeout_seconds: profile.cleanup_timeout_seconds,
        warm_template: None,
        regeneration_workflow: None,
        reap: profile.reap,
    }
}

// CORE-283: a mutation used to rewrite the document from its parsed form,
// which deleted every comment and reordered every key.
#[tokio::test]
async fn profile_set_replaces_one_value_and_leaves_every_other_byte_alone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let original = STARTER_CONFIG.replacen(
        "cpu_count = 4",
        "# Sized for the small mac.\ncpu_count = 4 # halved by hand",
        1,
    );
    let path = write_config(directory.path(), &original);

    run_profile_command(
        &path,
        ProfileCommand::Set(ProfileSetArgs {
            profile: profile_name("liftfg"),
            key: "cpu-count".to_owned(),
            value: "8".to_owned(),
        }),
    )
    .await
    .unwrap_or_else(|error| unreachable!("set: {error}"));

    assert_eq!(
        read(&path),
        original.replacen(
            "cpu_count = 4 # halved by hand",
            "cpu_count = 8 # halved by hand",
            1
        )
    );
}

/// The documented required-placeholder line, uncommented as an operator who
/// enrols guests would have it.
fn starter_with_a_required_placeholder() -> String {
    STARTER_CONFIG.replacen(
        "# preauth_key_file = \"${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}\"",
        "preauth_key_file = \"${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}\"",
        1,
    )
}

fn set_cpu_count(name: &str, value: &str) -> ProfileCommand {
    ProfileCommand::Set(ProfileSetArgs {
        profile: profile_name(name),
        key: "cpu-count".to_owned(),
        value: value.to_owned(),
    })
}

// CORE-284: a `${KEY}` the operator never touched refused the whole mutation
// whenever the invoking shell did not define it.
#[tokio::test]
async fn profile_set_saves_while_an_unrelated_placeholder_is_unset() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let original = starter_with_a_required_placeholder();
    let path = write_config(directory.path(), &original);

    run_profile_command(&path, set_cpu_count("liftfg", "8"))
        .await
        .unwrap_or_else(|error| unreachable!("set: {error}"));

    let saved = read(&path);
    assert_eq!(
        saved,
        original.replacen("cpu_count = 4", "cpu_count = 8", 1)
    );
    assert!(saved.contains("\"${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}\""));
}

// CORE-284: the deployment is deferred; the operator's own edit is not.
#[tokio::test]
async fn profile_set_still_refuses_an_invalid_edit_beside_a_placeholder() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let original = starter_with_a_required_placeholder();
    let path = write_config(directory.path(), &original);

    let error = run_profile_command(&path, set_cpu_count("liftfg", "0"))
        .await
        .err()
        .unwrap_or_else(|| unreachable!("an out-of-range CPU count must be refused"));
    assert!(
        format!("{error:#}").contains("outside the allowed range"),
        "refused for the wrong reason: {error:#}"
    );
    assert_eq!(read(&path), original);
}

#[tokio::test]
async fn profile_create_only_appends_the_new_table() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = write_config(directory.path(), STARTER_CONFIG);

    run_profile_command(
        &path,
        ProfileCommand::Create(Box::new(create_arguments("newton"))),
    )
    .await
    .unwrap_or_else(|error| unreachable!("create: {error}"));

    let saved = read(&path);
    let added = saved
        .strip_prefix(STARTER_CONFIG)
        .unwrap_or_else(|| unreachable!("existing document was rewritten:\n{saved}"));
    assert_eq!(
        added,
        r#"
[profiles.newton]
repository = "owner/project"
template = "flanforge-base"
runner_label = "macos-tart-project"
job_name = "apple-build"
allowed_workflows = ["apple.yml"]
allowed_events = ["push"]
allowed_refs = ["refs/heads/main"]
require_protected_ref = true
network = "softnet"
cpu_count = 4
memory_mb = 8192
storage_mb = 40960
boot_timeout_seconds = 30
idle_timeout_seconds = 30
job_timeout_seconds = 60
cleanup_timeout_seconds = 10
reap = true
"#
    );

    let config = flanforge_config::load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(config.ensure_valid(), Ok(()));
    assert!(config.profiles.contains_key(&profile_name("newton")));
}
