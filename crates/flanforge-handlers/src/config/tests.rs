use flanforge_wire::ConfigUpdate;

use super::*;
use crate::{WebuiFault, tests_support};

#[tokio::test]
async fn the_view_edits_and_conflicts_behave() {
    let (services, _directory) = tests_support::services().await;

    let view = get(&services)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    // The whole document is viewable; the schema carries editability.
    assert!(view.document.get("logging").is_some());
    assert!(view.document.get("oidc").is_some());
    assert!(view.effective.get("server").is_some());
    let issuer = view
        .schema
        .iter()
        .find(|field| field.path == "oidc.issuer")
        .unwrap_or_else(|| unreachable!("oidc.issuer missing from the schema"));
    assert!(!issuer.editable, "git OIDC stays view only");
    assert!(
        view.schema
            .iter()
            .any(|field| field.path == "profiles.*.hot.enabled" && field.editable)
    );

    let outcome = update(
        &services,
        "jim",
        &ConfigUpdate {
            version: view.version.clone(),
            changes: std::collections::BTreeMap::from([(
                "logging.level".to_owned(),
                serde_json::json!("debug"),
            )]),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("update: {error}"));
    assert_ne!(outcome.version, view.version);

    // The stale token is refused.
    assert!(matches!(
        update(
            &services,
            "jim",
            &ConfigUpdate {
                version: view.version,
                changes: std::collections::BTreeMap::from([(
                    "logging.level".to_owned(),
                    serde_json::json!("info"),
                )]),
            },
        )
        .await,
        Err(WebuiFault::Conflict(_))
    ));

    // A non-editable key is refused with its name.
    let refused = update(
        &services,
        "jim",
        &ConfigUpdate {
            version: outcome.version.clone(),
            changes: std::collections::BTreeMap::from([(
                "server.listen".to_owned(),
                serde_json::json!("0.0.0.0:1"),
            )]),
        },
    )
    .await;
    assert!(matches!(refused, Err(WebuiFault::Rejected(_))));

    // A value the schema refuses writes nothing.
    let invalid = update(
        &services,
        "jim",
        &ConfigUpdate {
            version: outcome.version,
            changes: std::collections::BTreeMap::from([(
                "runtime.poll_seconds".to_owned(),
                serde_json::json!(99_999),
            )]),
        },
    )
    .await;
    assert!(matches!(invalid, Err(WebuiFault::Rejected(_))));
}

#[tokio::test]
// One lifecycle told in order: create, refusals, then removal.
#[allow(clippy::too_many_lines)]
async fn profiles_are_created_and_removed_whole() {
    let (services, _directory) = tests_support::services().await;
    let view = get(&services)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    let profile = serde_json::json!({
        "repository": "owner/extra",
        "template": "flanforge-extra-base",
        "runner_label": "macos-tart-extra",
        "job_name": "extra-build",
        "allowed_workflows": ["ci.yml"],
        "allowed_events": ["push"],
        "allowed_refs": ["refs/heads/main"],
        "cpu_count": 2,
        "memory_mb": 4096,
        "boot_timeout_seconds": 30,
        "idle_timeout_seconds": 30,
        "job_timeout_seconds": 60,
        "cleanup_timeout_seconds": 10,
    });

    let created = profile_create(
        &services,
        "jim",
        "extra",
        &flanforge_wire::ProfileCreate {
            version: view.version.clone(),
            profile: profile.clone(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("create: {error}"));
    let after = get(&services)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert!(after.document["profiles"]["extra"].is_object());

    // A duplicate name and a mistyped field are both refused with the reason.
    assert!(matches!(
        profile_create(
            &services,
            "jim",
            "extra",
            &flanforge_wire::ProfileCreate {
                version: created.version.clone(),
                profile: profile.clone(),
            },
        )
        .await,
        Err(WebuiFault::Rejected(_))
    ));
    let mut mistyped = profile.clone();
    mistyped["cpu_count"] = serde_json::json!("two");
    assert!(matches!(
        profile_create(
            &services,
            "jim",
            "another",
            &flanforge_wire::ProfileCreate {
                version: created.version.clone(),
                profile: mistyped,
            },
        )
        .await,
        Err(WebuiFault::Rejected(_))
    ));

    // A stale token is refused; the current one removes the profile.
    assert!(matches!(
        profile_delete(
            &services,
            "jim",
            "extra",
            &flanforge_wire::ProfileDelete {
                version: view.version,
            },
        )
        .await,
        Err(WebuiFault::Conflict(_))
    ));
    let deleted = profile_delete(
        &services,
        "jim",
        "extra",
        &flanforge_wire::ProfileDelete {
            version: created.version,
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("delete: {error}"));
    assert!(deleted.hot_machines.is_empty());
    let after = get(&services)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert!(after.document["profiles"].get("extra").is_none());
    assert!(matches!(
        profile_delete(
            &services,
            "jim",
            "extra",
            &flanforge_wire::ProfileDelete {
                version: deleted.version,
            },
        )
        .await,
        Err(WebuiFault::Rejected(_))
    ));
}

/// A deployment whose configuration is a read-only mount still serves the
/// view, reports it as unwritable, and refuses edits with the reason.
#[tokio::test]
async fn a_read_only_config_is_viewable_and_refuses_edits() {
    let (services, directory) = tests_support::services().await;
    let restore = std::fs::metadata(directory.path())
        .unwrap_or_else(|error| unreachable!("metadata: {error}"))
        .permissions();
    let mut read_only = restore.clone();
    std::os::unix::fs::PermissionsExt::set_mode(&mut read_only, 0o555);
    std::fs::set_permissions(directory.path(), read_only)
        .unwrap_or_else(|error| unreachable!("chmod: {error}"));
    // Root ignores permission bits (CI runs as root), so the premise cannot
    // hold there; prove it bound before asserting on it.
    let probe = directory.path().join(".write-probe");
    if std::fs::write(&probe, b"x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(directory.path(), restore)
            .unwrap_or_else(|error| unreachable!("chmod back: {error}"));
        return;
    }

    let view = get(&services)
        .await
        .unwrap_or_else(|error| unreachable!("get: {error}"));
    assert!(!view.writable);
    let refused = update(
        &services,
        "jim",
        &ConfigUpdate {
            version: view.version,
            changes: std::collections::BTreeMap::from([(
                "logging.level".to_owned(),
                serde_json::json!("debug"),
            )]),
        },
    )
    .await;
    assert!(
        matches!(refused, Err(WebuiFault::Rejected(_))),
        "{refused:?}"
    );

    std::fs::set_permissions(directory.path(), restore)
        .unwrap_or_else(|error| unreachable!("chmod back: {error}"));
}
