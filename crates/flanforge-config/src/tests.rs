use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::*;

#[test]
fn environment_expansion_supports_required_and_default_values() {
    let lookup = |name: &str| match name {
        "USER" => Some("signing".to_owned()),
        "EMPTY" => Some(String::new()),
        _ => None,
    };
    assert_eq!(
        expand_string("/Users/${USER}/${MISSING:-runner}", &lookup)
            .unwrap_or_else(|error| unreachable!("expand: {error}")),
        "/Users/signing/runner"
    );
    assert_eq!(
        expand_string("${EMPTY:-fallback}", &lookup)
            .unwrap_or_else(|error| unreachable!("expand: {error}")),
        "fallback"
    );
    assert!(matches!(
        expand_string("${MISSING}", &lookup),
        Err(ConfigLoadError::MissingEnvironment(name)) if name == "MISSING"
    ));
}

#[test]
fn a_required_placeholder_fails_closed_under_an_empty_environment() {
    let value: toml::Value =
        toml::from_str("[tailscale]\npreauth_key_file = '${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}'")
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(matches!(
        decode_config(value, &|_| None, None),
        Err(ConfigLoadError::MissingEnvironment(name))
            if name == "FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE"
    ));
}

#[test]
fn expansion_is_data_and_cannot_add_toml_keys() {
    let mut value: toml::Value = toml::from_str("name = '${VALUE}'")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    expand_value(&mut value, &|_| Some("safe'\ninjected = true".to_owned()))
        .unwrap_or_else(|error| unreachable!("expand: {error}"));
    assert_eq!(value.get("injected"), None);
    assert_eq!(
        value.get("name").and_then(toml::Value::as_str),
        Some("safe'\ninjected = true")
    );
}

#[test]
fn malformed_placeholders_are_rejected() {
    for value in ["${}", "${9KEY}", "${BAD-NAME}", "${UNCLOSED"] {
        assert!(
            matches!(
                expand_string(value, &|_| Some("value".to_owned())),
                Err(ConfigLoadError::InvalidPlaceholder)
            ),
            "accepted {value}"
        );
    }
}

#[test]
fn tilde_paths_resolve_only_from_the_service_home() {
    let mut nested = PathBuf::from("~/Library/Logs/flanforged.log");
    resolve_home(&mut nested, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("resolve: {error}"));
    assert_eq!(
        nested,
        PathBuf::from("/Users/signing/Library/Logs/flanforged.log")
    );

    let mut other_user = PathBuf::from("~someone/file");
    resolve_home(&mut other_user, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("resolve: {error}"));
    assert_eq!(other_user, PathBuf::from("~someone/file"));

    let mut missing = PathBuf::from("~/file");
    assert!(matches!(
        resolve_home(&mut missing, None),
        Err(ConfigLoadError::HomeUnavailable)
    ));
}

#[test]
fn tailscale_key_path_resolves_from_the_service_home() {
    let mut config: flanforge_core::Config =
        toml::from_str(include_str!("../../../config.example.toml"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    config.tailscale.preauth_key_file = Some("~/Library/Secrets/headscale-key".into());
    let mut value =
        toml::Value::try_from(config).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    expand_value(&mut value, &|_| None).unwrap_or_else(|error| unreachable!("expand: {error}"));
    let resolved = decode_config(value, &|_| None, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("decode: {error}"));
    assert_eq!(
        resolved.tailscale.preauth_key_file,
        Some(PathBuf::from(
            "/Users/signing/Library/Secrets/headscale-key"
        ))
    );
}

#[test]
fn only_explicit_flat_tart_fields_are_legacy() {
    let legacy: toml::Value = toml::from_str(
        "[runtime]\nstate_dir = '/tmp/state'\nvm_prefix = 'ci-'\ntart_path = '/usr/bin/tart'",
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(is_legacy_runtime_document(&legacy));

    let implicit: toml::Value =
        toml::from_str("[runtime]\nstate_dir = '/tmp/state'\nvm_prefix = 'ci-'")
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(!is_legacy_runtime_document(&implicit));

    let canonical: toml::Value = toml::from_str(
        "[runtime]\nstate_dir = '/tmp/state'\nvm_prefix = 'ci-'\n[runtime.backend]\nkind = 'tart'",
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(!is_legacy_runtime_document(&canonical));
}

fn example_value() -> toml::Value {
    toml::from_str(include_str!("../../../config.example.toml"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn resolve_layers(
    value: toml::Value,
    environment: &[(String, String)],
    cli: &[(&str, &str)],
) -> flanforge_core::Config {
    decode_config_with_overrides(
        value,
        &|_| None,
        environment,
        cli,
        Some(Path::new("/Users/signing")),
    )
    .unwrap_or_else(|error| unreachable!("resolve: {error}"))
}

#[test]
fn precedence_is_defaults_then_toml_then_environment_then_cli() {
    let mut defaults = example_value();
    defaults
        .as_table_mut()
        .unwrap_or_else(|| unreachable!("root table"))
        .remove("logging");
    assert_eq!(resolve_layers(defaults, &[], &[]).logging.level, "info");

    let mut file = example_value();
    file["logging"]["level"] = toml::Value::String("warn".to_owned());
    assert_eq!(resolve_layers(file.clone(), &[], &[]).logging.level, "warn");

    let environment = [("logging.level".to_owned(), "debug".to_owned())];
    assert_eq!(
        resolve_layers(file.clone(), &environment, &[])
            .logging
            .level,
        "debug"
    );

    assert_eq!(
        resolve_layers(file, &environment, &[("logging.level", "trace")])
            .logging
            .level,
        "trace"
    );
}

#[test]
fn backend_and_paths_follow_the_same_typed_precedence_chain() {
    let environment = [
        ("runtime.backend.kind".to_owned(), "libvirt".to_owned()),
        (
            "runtime.backend.qemu_img_path".to_owned(),
            "/from/env/qemu-img".to_owned(),
        ),
    ];
    let config = resolve_layers(
        example_value(),
        &environment,
        &[("runtime.backend.qemu_img_path", "/from/cli/qemu-img")],
    );
    let backend = config
        .runtime
        .libvirt()
        .unwrap_or_else(|| unreachable!("libvirt override"));
    assert_eq!(backend.qemu_img_path, PathBuf::from("/from/cli/qemu-img"));
    assert_eq!(backend.pool, "flanforge");
}

#[test]
fn overrides_are_allowlisted_and_typed_without_echoing_values() {
    let Err(bad_key) = decode_config_with_overrides(
        example_value(),
        &|_| None,
        &[],
        &[("runtime.backend.shell", "secret-command")],
        Some(Path::new("/Users/signing")),
    ) else {
        unreachable!("unsupported key was accepted")
    };
    assert!(matches!(bad_key, ConfigLoadError::InvalidOverrideKey));
    assert!(!bad_key.to_string().contains("secret-command"));

    let Err(bad_value) = decode_config_with_overrides(
        example_value(),
        &|_| None,
        &[],
        &[("runtime.max_running_vms", "not-a-number")],
        Some(Path::new("/Users/signing")),
    ) else {
        unreachable!("invalid value was accepted")
    };
    assert!(matches!(
        bad_value,
        ConfigLoadError::InvalidOverrideValue(_)
    ));
    assert!(!bad_value.to_string().contains("not-a-number"));
}

#[test]
fn dynamic_profile_fields_support_typed_arrays_and_scalars() {
    let config = resolve_layers(
        example_value(),
        &[],
        &[
            ("profiles.liftfg.cpu_count", "6"),
            ("profiles.liftfg.job_name", "apple-release"),
        ],
    );
    let profile = config
        .profiles
        .get(
            &flanforge_core::ProfileName::new("liftfg")
                .unwrap_or_else(|error| unreachable!("profile name: {error}")),
        )
        .unwrap_or_else(|| unreachable!("profile"));
    assert_eq!(profile.cpu_count, 6);
    assert_eq!(profile.job_name.as_str(), "apple-release");

    // Typed arrays resolve through an override layer like any other field.
    let widened = resolve_layers(
        example_value(),
        &[],
        &[(
            "profiles.liftfg.allowed_events",
            "[\"push\", \"workflow_dispatch\"]",
        )],
    );
    let profile = widened
        .profiles
        .get(
            &flanforge_core::ProfileName::new("liftfg")
                .unwrap_or_else(|error| unreachable!("profile name: {error}")),
        )
        .unwrap_or_else(|| unreachable!("profile"));
    assert_eq!(profile.allowed_events.len(), 2);
}

// Removing a profile key is a breaking config change, and every section denies
// unknown fields — so the daemon must say which profile and what to write
// instead, not just "unknown field" (CORE-609).
#[test]
fn a_retired_profile_key_names_the_profile_and_its_replacement() {
    let mut value = example_value();
    value["profiles"]["liftfg"]
        .as_table_mut()
        .unwrap_or_else(|| unreachable!("fixture profile"))
        .insert(
            "allowed_ref_prefixes".to_owned(),
            toml::Value::try_from(["refs/tags/v"]).unwrap_or_else(|error| unreachable!("{error}")),
        );
    let Err(error) = decode_config(value, &|_| None, Some(Path::new("/Users/signing"))) else {
        unreachable!("a retired key loaded");
    };
    assert!(
        matches!(
            error,
            ConfigLoadError::RetiredProfileField {
                ref profile,
                field: "allowed_ref_prefixes",
                ..
            } if profile == "liftfg"
        ),
        "unexpected error: {error}"
    );
    let text = error.to_string();
    assert!(text.contains("liftfg"), "{text}");
    assert!(text.contains("allowed_refs"), "{text}");
    assert!(text.contains("refs/tags/v*"), "{text}");
}

/// The `[guest]` keys the channel work retired. Every section denies unknown
/// fields, so without this the daemon would stop with "unknown field" and no
/// hint of where the setting went.
#[test]
fn a_retired_guest_key_names_its_replacement() {
    for (retired, expected) in [
        ("ssh_user", "guest.runner_user"),
        ("ssh_identity_file", "[guest.ssh].identity_file"),
        ("ssh_known_hosts_file", "[guest.ssh].known_hosts_file"),
        ("ssh_host_key_alias", "[guest.ssh].host_key_alias"),
        (
            "ssh_connect_timeout_seconds",
            "[guest.ssh].connect_timeout_seconds",
        ),
        ("verify_host_key", "[guest.ssh].verify_host_key"),
    ] {
        let mut value = example_value();
        value["guest"]
            .as_table_mut()
            .unwrap_or_else(|| unreachable!("fixture guest table"))
            .insert(retired.to_owned(), toml::Value::String("x".to_owned()));
        let Err(error) = decode_config(value, &|_| None, Some(Path::new("/Users/signing"))) else {
            unreachable!("retired key {retired} loaded");
        };
        let text = error.to_string();
        assert!(text.contains(retired), "{text}");
        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn canonical_view_moves_legacy_tart_keys_without_expanding_values() {
    let mut value: toml::Value = toml::from_str(
        r#"
        [runtime]
        state_dir = "${STATE_DIR}"
        vm_prefix = "ci-"
        tart_path = "${TART_PATH}"
        tart_home = "~/.tart"
        forgejo_runner_host_path = "~/bin/forgejo-runner"
        "#,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    canonicalize_runtime(&mut value);
    let runtime = value["runtime"]
        .as_table()
        .unwrap_or_else(|| unreachable!("runtime"));
    assert!(!runtime.contains_key("tart_path"));
    assert!(!runtime.contains_key("tart_home"));
    assert!(!runtime.contains_key("forgejo_runner_host_path"));
    assert_eq!(runtime["backend"]["kind"].as_str(), Some("tart"));
    assert_eq!(runtime["backend"]["path"].as_str(), Some("${TART_PATH}"));
    assert_eq!(runtime["state_dir"].as_str(), Some("${STATE_DIR}"));
}

fn tart_runtime_value() -> toml::Value {
    let mut value = example_value();
    value["runtime"]["backend"]["path"] = toml::Value::String("/opt/custom/tart".to_owned());
    value["runtime"]["backend"]["home"] = toml::Value::String("/var/lib/tart".to_owned());
    // The example omits the key (baked-runner default), so insert rather
    // than index it.
    value["runtime"]["backend"]
        .as_table_mut()
        .unwrap_or_else(|| unreachable!("backend table"))
        .insert(
            "runner_host_path".to_owned(),
            toml::Value::String("/opt/custom/forgejo-runner".to_owned()),
        );
    value
}

fn legacy_runtime_value() -> toml::Value {
    let mut value = example_value();
    let runtime = value["runtime"]
        .as_table_mut()
        .unwrap_or_else(|| unreachable!("runtime table"));
    runtime.remove("backend");
    for (key, path) in [
        ("tart_path", "/legacy/bin/tart"),
        ("tart_home", "/legacy/tart"),
        ("forgejo_runner_host_path", "/legacy/bin/forgejo-runner"),
    ] {
        runtime.insert(key.to_owned(), toml::Value::String(path.to_owned()));
    }
    value
}

#[test]
fn restating_the_declared_backend_kind_keeps_its_configured_values() {
    let value = tart_runtime_value();
    let assert_preserved = |environment: &[(String, String)], cli: &[(&str, &str)]| {
        let tart = resolve_layers(value.clone(), environment, cli)
            .runtime
            .tart()
            .cloned()
            .unwrap_or_else(|| unreachable!("Tart backend"));
        assert_eq!(tart.path, PathBuf::from("/opt/custom/tart"));
        assert_eq!(tart.home, Some(PathBuf::from("/var/lib/tart")));
        assert_eq!(
            tart.runner_host_path,
            Some(PathBuf::from("/opt/custom/forgejo-runner"))
        );
    };
    let restatement = [("runtime.backend.kind".to_owned(), "tart".to_owned())];
    assert_preserved(&restatement, &[]);
    assert_preserved(&[], &[("runtime.backend.kind", "tart")]);
    assert_preserved(&restatement, &[("runtime.backend.kind", "tart")]);
}

#[test]
fn a_later_layer_restating_the_kind_keeps_the_earlier_layer_backend_values() {
    let environment = [
        ("runtime.backend.kind".to_owned(), "libvirt".to_owned()),
        ("runtime.backend.pool".to_owned(), "custom-pool".to_owned()),
    ];
    let libvirt = resolve_layers(
        example_value(),
        &environment,
        &[("runtime.backend.kind", "libvirt")],
    )
    .runtime
    .libvirt()
    .cloned()
    .unwrap_or_else(|| unreachable!("libvirt backend"));
    assert_eq!(libvirt.pool, "custom-pool");
}

#[test]
fn the_backend_kind_override_migrates_a_legacy_runtime_document() {
    let value = legacy_runtime_value();
    assert!(is_legacy_runtime_document(&value));
    // The model refuses legacy keys beside a backend table, so decoding at all
    // proves the override stripped them.
    let tart = resolve_layers(value, &[], &[("runtime.backend.kind", "tart")])
        .runtime
        .tart()
        .cloned()
        .unwrap_or_else(|| unreachable!("Tart backend"));
    assert_eq!(tart.path, flanforge_paths::default_tart_path());
    // The kind override resets the backend table to its defaults, and the
    // default delivery is the image-baked runner.
    assert_eq!(tart.runner_host_path, None);
    assert_eq!(tart.home, None);
}

#[test]
fn a_different_backend_kind_still_replaces_the_declared_table() {
    let libvirt = resolve_layers(
        tart_runtime_value(),
        &[],
        &[("runtime.backend.kind", "libvirt")],
    )
    .runtime
    .libvirt()
    .cloned()
    .unwrap_or_else(|| unreachable!("libvirt backend"));
    assert_eq!(libvirt.uri, flanforge_core::LIBVIRT_SYSTEM_URI);
    assert_eq!(libvirt.pool, "flanforge");
    assert_eq!(libvirt.network, "flanforge-ci");
}

#[test]
fn empty_environment_and_cli_values_clear_valid_strings() {
    let mut value = example_value();
    value["tailscale"]["extra_args"] = toml::Value::String("--accept-routes".to_owned());

    let from_environment = resolve_layers(
        value.clone(),
        &[("tailscale.extra_args".to_owned(), String::new())],
        &[],
    );
    assert_eq!(from_environment.tailscale.extra_args, "");

    let from_cli = resolve_layers(
        value,
        &[(
            "tailscale.extra_args".to_owned(),
            "--accept-routes".to_owned(),
        )],
        &[("tailscale.extra_args", "")],
    );
    assert_eq!(from_cli.tailscale.extra_args, "");
}

#[test]
fn environment_collection_preserves_actual_empty_values() {
    let environment = [(
        OsString::from("FLANFORGE__TAILSCALE__EXTRA_ARGS"),
        OsString::new(),
    )];
    assert_eq!(
        crate::overlay::collect_environment_overrides(environment)
            .unwrap_or_else(|error| unreachable!("environment: {error}")),
        vec![("tailscale.extra_args".to_owned(), String::new())]
    );
}

#[test]
fn native_service_install_rejects_even_empty_ambient_overrides() {
    assert!(ensure_service_overrides_are_durable(&[]).is_ok());
    assert!(matches!(
        ensure_service_overrides_are_durable(&[("tailscale.extra_args".to_owned(), String::new())]),
        Err(ConfigLoadError::AmbientServiceOverrides)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn config_open_rejects_a_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let target = directory.path().join("target.toml");
    let link = directory.path().join("config.toml");
    std::fs::write(&target, "value = true\n")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    symlink(&target, &link).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert!(matches!(
        ConfigDocument::open(&link).await,
        Err(ConfigLoadError::NotRegular)
    ));
}

#[tokio::test]
async fn config_open_rejects_a_directory() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    assert!(matches!(
        ConfigDocument::open(directory.path()).await,
        Err(ConfigLoadError::NotRegular)
    ));
}

// CORE-283: saving edits the operator's file instead of re-rendering it from
// the parsed value, which dropped every comment and reordered every key.
#[tokio::test]
async fn saving_edits_one_value_and_keeps_the_comments_and_key_order() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    std::fs::write(&path, STARTER_CONFIG).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let mut formatted = document.formatted().clone();
    formatted["server"]["shutdown_grace_seconds"] = toml_edit::value(45);
    document
        .save(formatted)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));

    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("read: {error}")),
        STARTER_CONFIG.replacen(
            "shutdown_grace_seconds = 30",
            "shutdown_grace_seconds = 45",
            1
        )
    );
    assert_eq!(
        document.raw()["server"]["shutdown_grace_seconds"].as_integer(),
        Some(45)
    );
}

#[tokio::test]
async fn saving_a_document_that_no_longer_validates_leaves_the_file_alone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    std::fs::write(&path, STARTER_CONFIG).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let mut formatted = document.formatted().clone();
    formatted["profiles"]["liftfg"]["cpu_count"] = toml_edit::value(0);

    assert!(matches!(
        document.save(formatted).await,
        Err(ConfigLoadError::Validation(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("read: {error}")),
        STARTER_CONFIG
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

// CORE-284: a placeholder the edit never touched refused the whole write, so
// the operator hand-edited the TOML and skipped validation altogether.
#[tokio::test]
async fn saving_defers_a_placeholder_the_edit_did_not_touch() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    let original = starter_with_a_required_placeholder();
    std::fs::write(&path, &original).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let mut formatted = document.formatted().clone();
    formatted["profiles"]["liftfg"]["cpu_count"] = toml_edit::value(8);
    document
        .save(formatted)
        .await
        .unwrap_or_else(|error| unreachable!("save: {error}"));

    let saved = std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        saved,
        original.replacen("cpu_count = 4", "cpu_count = 8", 1)
    );
    assert!(saved.contains("\"${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}\""));
}

// CORE-284: deferring the deployment must not defer the operator's own edit.
#[tokio::test]
async fn a_deferred_placeholder_does_not_excuse_an_invalid_edit() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    let original = starter_with_a_required_placeholder();
    std::fs::write(&path, &original).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let mut formatted = document.formatted().clone();
    formatted["profiles"]["liftfg"]["cpu_count"] = toml_edit::value(0);

    assert!(matches!(
        document.save(formatted).await,
        Err(ConfigLoadError::Validation(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("{error}")),
        original
    );
}

// CORE-284: a placeholder the mutation is responsible for still fails, and
// names the profile that holds it.
#[tokio::test]
async fn an_unresolved_placeholder_inside_a_profile_names_it() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    std::fs::write(&path, STARTER_CONFIG).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let mut formatted = document.formatted().clone();
    formatted["profiles"]["liftfg"]["template"] = toml_edit::value("${FLANFORGE_BASE_TEMPLATE}");

    assert!(matches!(
        document.save(formatted).await,
        Err(ConfigLoadError::UnresolvedProfilePlaceholder { profile, variable })
            if profile == "liftfg" && variable == "FLANFORGE_BASE_TEMPLATE"
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("{error}")),
        STARTER_CONFIG
    );
}

#[tokio::test]
async fn config_open_bounds_the_descriptor_read() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    let bytes = usize::try_from(MAX_CONFIG_BYTES)
        .unwrap_or_else(|_| unreachable!("config byte bound fits usize"));
    std::fs::write(&path, vec![b'a'; bytes + 1])
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(matches!(
        ConfigDocument::open(&path).await,
        Err(ConfigLoadError::TooLarge)
    ));
}

#[test]
fn typed_schema_and_override_mapping_are_exhaustive() {
    let values = populated_config_values();
    let actual = values
        .iter()
        .flat_map(schema_leaf_paths)
        .map(|path| normalize_profile_path(&path))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, crate::overlay::supported_field_patterns());
}

#[test]
fn every_typed_leaf_is_toml_environment_and_cli_addressable() {
    for value in populated_config_values() {
        let round_trip = value.clone().try_into::<flanforge_core::Config>();
        assert!(
            round_trip.is_ok(),
            "typed TOML did not round-trip: {round_trip:?}"
        );

        for (key, field) in config_leaves(&value) {
            let raw = override_text(&field);
            let environment_name = format!(
                "{}{}",
                crate::overlay::ENV_PREFIX,
                key.replace('.', "__").to_ascii_uppercase()
            );
            assert_eq!(
                crate::overlay::environment_key(&environment_name).as_deref(),
                Some(key.as_str())
            );

            let environment = [(key.clone(), raw.clone())];
            decode_config_with_overrides(
                value.clone(),
                &|_| None,
                &environment,
                &[],
                Some(Path::new("/Users/service")),
            )
            .unwrap_or_else(|error| unreachable!("environment field {key}: {error}"));

            decode_config_with_overrides(
                value.clone(),
                &|_| None,
                &[],
                &[(key.as_str(), raw.as_str())],
                Some(Path::new("/Users/service")),
            )
            .unwrap_or_else(|error| unreachable!("CLI field {key}: {error}"));
        }
    }
}

fn populated_config_values() -> Vec<toml::Value> {
    let mut tart = (*flanforge_test_support::config("/var/lib/flanforge-test".into())).clone();
    tart.logging.path = Some("/var/log/flanforged.log".into());
    tart.runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
        .home = Some("/var/lib/tart".into());
    tart.db.db_path = Some("/var/lib/flanforge-test/flanforge.db".into());
    tart.webui.dev_dist_dir = Some("/private/webui-dist".into());
    tart.webui.oidc.enabled = true;
    tart.webui.oidc.issuer = Some(
        "https://idp.example.test"
            .parse()
            .unwrap_or_else(|error| unreachable!("URL: {error}")),
    );
    tart.webui.oidc.client_id = Some("flanforge-webui".to_owned());
    tart.webui.oidc.client_secret_file = Some("/private/webui-oidc-secret".into());
    tart.webui.oidc.redirect_url = Some(
        "https://ci.example.test/api/v1/oidc/callback"
            .parse()
            .unwrap_or_else(|error| unreachable!("URL: {error}")),
    );
    tart.tailscale.preauth_key_file = Some("/private/tailscale-key".into());
    tart.tailscale.login_server = Some(
        "https://login.example.test"
            .parse()
            .unwrap_or_else(|error| unreachable!("URL: {error}")),
    );
    tart.tailscale.hostname = Some("runner.example.test".to_owned());
    // One slot, inside every hot range, so the ungated range rules pass on a
    // fixture whose only job is to make each leaf addressable.
    tart.runtime.max_hot_vms = 1;
    if let Some(profile) = tart.profiles.values_mut().next() {
        profile.warm_template = Some(
            flanforge_core::VmName::new("project-warm")
                .unwrap_or_else(|error| unreachable!("VM: {error}")),
        );
        profile.regeneration_workflow = Some("apple.yml".to_owned());
        profile.hot = Some(flanforge_core::HotConfig::default());
    }

    let mut libvirt = tart.clone();
    libvirt.runtime.backend =
        flanforge_core::RuntimeBackendConfig::Libvirt(flanforge_core::LibvirtConfig {
            image_manifest_dir: "/var/lib/flanforge-test/libvirt/published-bases".into(),
            image_import_dir: Some("/private/import-images".into()),
            ..flanforge_core::LibvirtConfig::default()
        });
    libvirt.runtime.max_running_vms = 1;
    libvirt.runtime.host_cpu_count = Some(8);
    libvirt.runtime.host_memory_mb = Some(32_768);
    libvirt.runtime.host_storage_mb = Some(262_144);
    libvirt.guest.channel = flanforge_core::GuestChannelKind::Agent;
    // One fixture exercises the SSH-less shape the agent channel admits; the
    // Tart fixture above is what makes every `[guest.ssh]` leaf addressable.
    libvirt.guest.ssh = None;
    for profile in libvirt.profiles.values_mut() {
        profile.network = flanforge_core::NetworkMode::Default;
        profile.warm_template = None;
        profile.regeneration_workflow = None;
    }

    [tart, libvirt]
        .into_iter()
        .map(|config| {
            toml::Value::try_from(config)
                .unwrap_or_else(|error| unreachable!("serialize config: {error}"))
        })
        .collect()
}

fn schema_leaf_paths(value: &toml::Value) -> Vec<String> {
    config_leaves(value)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

fn config_leaves(value: &toml::Value) -> Vec<(String, toml::Value)> {
    let mut leaves = Vec::new();
    collect_leaves(value, "", &mut leaves);
    leaves
}

fn collect_leaves(value: &toml::Value, path: &str, leaves: &mut Vec<(String, toml::Value)>) {
    if let Some(table) = value.as_table() {
        for (field, value) in table {
            let field = if path == "profiles" { "project" } else { field };
            let child = if path.is_empty() {
                field.to_owned()
            } else {
                format!("{path}.{field}")
            };
            collect_leaves(value, &child, leaves);
        }
    } else {
        leaves.push((path.to_owned(), value.clone()));
    }
}

fn normalize_profile_path(path: &str) -> String {
    let mut segments = path.split('.').collect::<Vec<_>>();
    if segments.first() == Some(&"profiles") && segments.len() >= 3 {
        segments[1] = "*";
    }
    segments.join(".")
}

fn override_text(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

#[test]
fn every_fixed_ui_editable_key_exists_in_the_typed_schema() {
    let patterns = crate::overlay::supported_field_patterns();
    for key in flanforge_core::ui_editable_fixed_keys() {
        assert!(patterns.contains(*key), "{key} is not typed");
        let segments: Vec<&str> = key.split('.').collect();
        assert!(flanforge_core::is_ui_editable(&segments), "{key}");
    }
    // Representative prefix keys resolve to a kind too.
    for key in [
        "tailscale.enabled",
        "guest.runner_user",
        "guest.ssh.identity_file",
        "profiles.any.cpu_count",
        "profiles.any.hot.enabled",
    ] {
        let segments: Vec<&str> = key.split('.').collect();
        assert!(flanforge_core::is_ui_editable(&segments), "{key}");
        assert!(crate::overlay::field_kind(&segments).is_some(), "{key}");
    }
    // The exclusions hold.
    for key in [
        "server.listen",
        "oidc.issuer",
        "db.db_path",
        "webui.oidc.enabled",
        "webui.authdb.enabled",
        "webui.public_read_only",
        "runtime.state_dir",
        "runtime.backend.uri",
        "forgejo.api_url",
    ] {
        let segments: Vec<&str> = key.split('.').collect();
        assert!(!flanforge_core::is_ui_editable(&segments), "{key}");
    }
}

#[tokio::test]
async fn edits_apply_atomically_with_versioning_and_the_allowlist() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, crate::STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));

    let document = EditableDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let version = document.version();
    // The editable view holds allowlisted leaves and nothing sensitive.
    let view = document.editable_view(flanforge_core::is_ui_editable);
    let table = view.as_table().unwrap_or_else(|| unreachable!("table"));
    assert!(table.contains_key("logging"));
    assert!(table.contains_key("profiles"));
    assert!(!table.contains_key("oidc"));
    assert!(
        !table.contains_key("forgejo") || {
            let forgejo = table["forgejo"]
                .as_table()
                .unwrap_or_else(|| unreachable!());
            forgejo.len() == 1 && forgejo.contains_key("http_timeout_seconds")
        }
    );

    let changes = std::collections::BTreeMap::from([
        (
            "logging.level".to_owned(),
            EditValue::String("debug".to_owned()),
        ),
        ("runtime.poll_seconds".to_owned(), EditValue::Integer(3)),
    ]);
    document
        .apply(&version, &changes, |segments| {
            flanforge_core::is_ui_editable(segments)
        })
        .await
        .unwrap_or_else(|error| unreachable!("apply: {error}"));
    let saved = tokio::fs::read_to_string(&path)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(saved.contains("level = \"debug\""));
    assert!(saved.contains("poll_seconds = 3"));

    // The stale version is refused; nothing changes.
    let stale = EditableDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    assert!(matches!(
        stale
            .apply(&version, &changes, |segments| {
                flanforge_core::is_ui_editable(segments)
            })
            .await,
        Err(ConfigEditError::VersionConflict)
    ));

    // A non-editable key is refused before anything is applied.
    let document = EditableDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let version = document.version();
    let refused = std::collections::BTreeMap::from([(
        "server.listen".to_owned(),
        EditValue::String("0.0.0.0:1".to_owned()),
    )]);
    assert!(matches!(
        document
            .apply(&version, &refused, |segments| {
                flanforge_core::is_ui_editable(segments)
            })
            .await,
        Err(ConfigEditError::NotEditable { .. })
    ));

    // An edit that fails validation writes nothing.
    let document = EditableDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    let version = document.version();
    let invalid = std::collections::BTreeMap::from([(
        "runtime.poll_seconds".to_owned(),
        EditValue::Integer(99_999),
    )]);
    assert!(matches!(
        document
            .apply(&version, &invalid, |segments| {
                flanforge_core::is_ui_editable(segments)
            })
            .await,
        Err(ConfigEditError::Load(ConfigLoadError::Validation(_)))
    ));
    let unchanged = tokio::fs::read_to_string(&path)
        .await
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(unchanged, saved);
}
