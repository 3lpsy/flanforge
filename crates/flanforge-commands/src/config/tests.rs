use std::path::{Path, PathBuf};

use flanforge_config::{LIBVIRT_STARTER_CONFIG, STARTER_CONFIG};
use flanforge_core::ProfileName;
use toml::Value;

use flanforge_cli::{ConfigBackend, ConfigGenerateArgs, ConfigViewArgs};

use super::{
    generate::generate,
    view::{view, viewed},
};

fn arguments(output: Option<PathBuf>, force: bool) -> ConfigGenerateArgs {
    ConfigGenerateArgs {
        backend: ConfigBackend::Tart,
        output,
        force,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| unreachable!("read: {error}"))
}

#[tokio::test]
async fn libvirt_generation_selects_a_linux_valid_starter() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    let arguments = ConfigGenerateArgs {
        backend: ConfigBackend::Libvirt,
        output: Some(path.clone()),
        force: false,
    };

    generate(&path, &arguments)
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    assert_eq!(read(&path), LIBVIRT_STARTER_CONFIG);
    let config = flanforge_config::load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(config.ensure_valid(), Ok(()));
    assert_eq!(
        config.runtime.backend_kind(),
        flanforge_core::RuntimeBackendKind::Libvirt
    );
}

#[tokio::test]
async fn a_generated_document_is_the_shipped_example_and_loads() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("nested/config.toml");

    generate(&path, &arguments(None, false))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    assert_eq!(read(&path), STARTER_CONFIG);
    let config = flanforge_config::load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(config.ensure_valid(), Ok(()));
    assert!(!config.profiles.is_empty());
}

#[tokio::test]
async fn an_existing_document_survives_a_second_run_without_force() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    let original = "# operator edited\n";
    std::fs::write(&path, original).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let error = generate(&path, &arguments(Some(path.clone()), false))
        .await
        .err()
        .unwrap_or_else(|| unreachable!("overwrite must fail"));

    assert!(error.to_string().contains("--force"), "{error}");
    assert_eq!(read(&path), original);
}

#[tokio::test]
async fn force_replaces_an_existing_document() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "# operator edited\n")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    generate(&path, &arguments(Some(path.clone()), true))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    assert_eq!(read(&path), STARTER_CONFIG);
}

#[cfg(unix)]
#[tokio::test]
async fn a_generated_document_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");

    generate(&path, &arguments(Some(path.clone()), false))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    let metadata =
        std::fs::metadata(&path).unwrap_or_else(|error| unreachable!("metadata: {error}"));
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn a_relative_destination_is_rejected_before_any_write() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");

    assert!(
        generate(&path, &arguments(Some(PathBuf::from("config.toml")), false))
            .await
            .is_err()
    );
    assert!(!path.exists());
}

/// Omits `[tailscale]` and `server.request_body_limit_bytes` so the resolved
/// view has defaults to supply, and leaves one placeholder unexpanded.
const VIEW_FIXTURE: &str = r#"
[logging]
level = "info"
path = "${HOME}/flanforged.log"

[server]
listen = "127.0.0.1:9843"

[oidc]
issuer = "https://git.example.com/api/actions"
audience = "flanforged"
jwks_url = "https://git.example.com/api/actions/.well-known/keys"

[forgejo]
api_url = "https://git.example.com/api/v1/"
api_token_file = "/etc/flanforge/api-token"

[runtime]
state_dir = "/var/lib/flanforge"
vm_prefix = "ci-"

[runtime.backend]
kind = "libvirt"
uri = "qemu:///system"
pool = "flanforge"
network = "flanforge-ci"

[guest]
runner_user = "runner"
forgejo_runner_path = "/home/runner/bin/forgejo-runner"

[profiles.demo]
repository = "owner/demo"
template = "flanforge-base"
runner_label = "linux-libvirt-demo"
job_name = "build"
allowed_workflows = ["build.yml"]
allowed_events = ["push"]
allowed_refs = ["refs/heads/main"]
cpu_count = 2
memory_mb = 4096
boot_timeout_seconds = 300
idle_timeout_seconds = 600
job_timeout_seconds = 3600
cleanup_timeout_seconds = 120
"#;

fn view_arguments() -> ConfigViewArgs {
    ConfigViewArgs {
        resolved: false,
        logging: false,
        server: false,
        oidc: false,
        forgejo: false,
        runtime: false,
        guest: false,
        tailscale: false,
        profiles: false,
        profile: None,
    }
}

fn resolved_arguments() -> ConfigViewArgs {
    ConfigViewArgs {
        resolved: true,
        ..view_arguments()
    }
}

fn write_document(directory: &Path, text: &str) -> PathBuf {
    let path = directory.join("config.toml");
    std::fs::write(&path, text).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    path
}

fn nested<'a>(value: &'a Value, section: &str, key: &str) -> Option<&'a Value> {
    value.get(section).and_then(|section| section.get(key))
}

#[tokio::test]
async fn the_resolved_view_supplies_a_default_the_document_omits() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = write_document(directory.path(), VIEW_FIXTURE);

    let raw = viewed(&path, &view_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("raw view: {error}"));
    let resolved = viewed(&path, &resolved_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("resolved view: {error}"));

    assert_eq!(nested(&raw, "server", "request_body_limit_bytes"), None);
    assert_eq!(
        nested(&resolved, "server", "request_body_limit_bytes"),
        Some(&Value::Integer(4_096))
    );
    assert_eq!(
        nested(&resolved, "oidc", "jwks_cache_seconds"),
        Some(&Value::Integer(300))
    );
}

#[tokio::test]
async fn the_resolved_view_expands_a_placeholder_the_raw_view_leaves_literal() {
    let home = std::env::var("HOME").unwrap_or_else(|error| unreachable!("HOME: {error}"));
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = write_document(directory.path(), VIEW_FIXTURE);

    let raw = viewed(&path, &view_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("raw view: {error}"));
    let resolved = viewed(&path, &resolved_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("resolved view: {error}"));

    assert_eq!(
        nested(&raw, "logging", "path"),
        Some(&Value::String("${HOME}/flanforged.log".to_owned()))
    );
    assert_eq!(
        nested(&resolved, "logging", "path"),
        Some(&Value::String(format!("{home}/flanforged.log")))
    );
}

#[tokio::test]
async fn the_resolved_view_composes_with_a_section_selector() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = write_document(directory.path(), VIEW_FIXTURE);
    let section = |resolved: bool| ConfigViewArgs {
        resolved,
        tailscale: true,
        ..view_arguments()
    };

    let error = viewed(&path, &section(false))
        .await
        .err()
        .unwrap_or_else(|| unreachable!("an absent section must fail"));
    let resolved = viewed(&path, &section(true))
        .await
        .unwrap_or_else(|error| unreachable!("resolved section: {error}"));

    assert!(
        error
            .to_string()
            .contains("section tailscale does not exist"),
        "{error}"
    );
    assert_eq!(
        resolved.as_table().map(toml::Table::len),
        Some(1),
        "the selector must narrow the resolved document"
    );
    assert_eq!(
        nested(&resolved, "tailscale", "enabled"),
        Some(&Value::Boolean(false))
    );
}

#[tokio::test]
async fn the_resolved_view_narrows_to_one_profile_and_renders() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = write_document(directory.path(), VIEW_FIXTURE);
    let profile = ProfileName::new("demo").unwrap_or_else(|error| unreachable!("name: {error}"));
    let arguments = ConfigViewArgs {
        profile: Some(profile),
        ..resolved_arguments()
    };

    let selected = viewed(&path, &arguments)
        .await
        .unwrap_or_else(|error| unreachable!("resolved profile: {error}"));

    let demo = selected
        .get("profiles")
        .and_then(|profiles| profiles.get("demo"))
        .unwrap_or_else(|| unreachable!("the selected profile must be present"));
    assert_eq!(demo.get("storage_mb"), Some(&Value::Integer(40_960)));
    assert_eq!(demo.get("reap"), Some(&Value::Boolean(true)));
    // The whole resolved document must also survive TOML rendering.
    view(&path, &resolved_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("render: {error}"));
}

#[tokio::test]
async fn an_unresolvable_placeholder_names_the_variable_instead_of_falling_back() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let text = VIEW_FIXTURE.replace("${HOME}", "${FLANFORGE_VIEW_TEST_ABSENT}");
    let path = write_document(directory.path(), &text);

    let error = viewed(&path, &resolved_arguments())
        .await
        .err()
        .unwrap_or_else(|| unreachable!("an unresolvable placeholder must fail"));

    let message = format!("{error:#}");
    assert!(
        message.contains("cannot resolve configuration"),
        "{message}"
    );
    assert!(
        message.contains("FLANFORGE_VIEW_TEST_ABSENT"),
        "{message}, which does not name the variable"
    );
    // The raw view is unaffected: resolution failure is never a silent fallback.
    viewed(&path, &view_arguments())
        .await
        .unwrap_or_else(|error| unreachable!("raw view: {error}"));
}
