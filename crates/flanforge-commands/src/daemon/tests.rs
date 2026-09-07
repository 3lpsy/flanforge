use std::path::{Path, PathBuf};

use flanforge_config::ConfigOverrides;
use flanforge_test_support::capture_logs;

use super::status::{print_service_status, unavailable_line};

/// Sixty-four graphic bytes, shaped like the credential the daemon writes.
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn native_service_commands_remain_provider_backed() {
    let _service = super::service::native();
}

/// A configuration whose state directory is the fixture's, so a test decides
/// what credential — if any — the command finds there.
fn config_fixture(directory: &Path) -> PathBuf {
    let path = directory.join("config.toml");
    let state_dir = directory.join("state");
    std::fs::create_dir_all(&state_dir).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::write(
        &path,
        flanforge_config::STARTER_CONFIG.replace(
            r#"state_dir = "~/Library/Application Support/flanforge/state""#,
            &format!(r#"state_dir = "{}""#, state_dir.display()),
        ),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    path
}

fn write_token(directory: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join("state").join("operator.token");
    std::fs::write(&path, TOKEN).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
}

/// Reports the service section for one fixture and returns what it logged.
fn report(config_path: &Path) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let ((), logged) = capture_logs(|| {
        runtime.block_on(print_service_status(
            config_path,
            &ConfigOverrides::default(),
        ));
    });
    logged
}

/// A credential the reader refuses once ended the report in silence, which an
/// operator reads as a healthy service with nothing to add.
#[test]
fn a_credential_refused_for_its_mode_still_reports_why() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config_path = config_fixture(directory.path());
    write_token(directory.path(), 0o644);

    let logged = report(&config_path);

    assert!(logged.contains("WARN"), "{logged}");
    assert!(logged.contains("readable beyond its owner"), "{logged}");
    assert!(
        logged.contains("check its mode and the account this command runs as"),
        "{logged}"
    );
    assert!(
        !logged.contains(TOKEN),
        "the credential reached the report: {logged}"
    );
}

/// The absent case names its path, and stays the one case that suggests the
/// service is simply not running.
#[test]
fn an_absent_credential_still_reports_why() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config_path = config_fixture(directory.path());

    let logged = report(&config_path);

    assert!(logged.contains("no operator credential at"), "{logged}");
    assert!(logged.contains("is the service running?"), "{logged}");
}

/// Two different failures: one daemon answered nothing, the other could never
/// be asked. Both keep the report's columns.
#[test]
fn the_unavailable_line_is_not_the_not_answering_line() {
    let line = unavailable_line("cannot read the operator credential");

    assert_eq!(
        line,
        "service:    unavailable; cannot read the operator credential"
    );
    assert!(line.starts_with("service:    "), "{line}");
}
