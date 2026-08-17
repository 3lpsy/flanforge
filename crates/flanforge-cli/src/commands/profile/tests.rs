use std::collections::BTreeSet;

use toml::Value;

use super::parse::{parse_string_list, parse_value};

#[test]
fn profile_set_values_are_typed_and_cannot_inject_toml() {
    assert_eq!(parse_value("cpu_count", "8").ok(), Some(Value::Integer(8)));
    assert!(parse_value("cpu_count", "8\nevil = true").is_err());
    assert!(parse_string_list("[\"main\"]\nevil = true").is_err());
    assert_eq!(
        parse_string_list("push, workflow_dispatch").ok(),
        Some(Value::Array(vec![
            Value::String("push".into()),
            Value::String("workflow_dispatch".into())
        ]))
    );
}

#[test]
fn all_profile_keys_are_explicitly_allowlisted() {
    let expected = BTreeSet::from([
        "allowed_events",
        "allowed_ref_prefixes",
        "allowed_refs",
        "allowed_workflows",
        "boot_timeout_seconds",
        "cleanup_timeout_seconds",
        "cpu_count",
        "idle_timeout_seconds",
        "job_name",
        "job_timeout_seconds",
        "memory_mb",
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
            | "boot_timeout_seconds"
            | "cleanup_timeout_seconds"
            | "idle_timeout_seconds"
            | "job_timeout_seconds" => "1",
            _ => "value",
        };
        assert!(parse_value(key, sample).is_ok(), "missing key {key}");
    }
    assert!(parse_value("command", "touch /tmp/host").is_err());
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
