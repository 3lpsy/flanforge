use std::os::unix::fs::PermissionsExt;

use url::Url;

use super::*;
use crate::guest::GuestSession;

/// The unpinned session every Tart guest is driven through.
fn session() -> GuestSession {
    GuestSession::configured("127.0.0.1").unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

#[test]
fn script_uses_file_credential_and_quotes_each_argument() {
    let script = tailscale_script(
        TART_TAILSCALE_PATH,
        "https://headscale.example/",
        Some("ci-guest"),
        &[
            "--accept-dns=false".into(),
            "--advertise-tags=tag:ci".into(),
            "--hostname=$(touch /tmp/host)".into(),
        ],
        "runner",
    );
    assert!(script.contains("mktemp '/tmp/flanforged-tailscale-preauth-key.XXXXXX'"));
    assert!(script.contains("\"--auth-key=file:$key_path\""));
    assert!(script.contains("chmod 0600"));
    assert!(script.contains("'--login-server=https://headscale.example/'"));
    assert!(script.contains("'--hostname=ci-guest'"));
    assert!(script.contains("'--accept-dns=false'"));
    assert!(script.contains("'--advertise-tags=tag:ci'"));
    assert!(script.contains("'--hostname=$(touch /tmp/host)'"));
    assert!(!script.contains("--auth-key=$key"));
    assert!(script.contains("unset key"));
    assert!(script.contains("trap cleanup EXIT"));
    let extra_hostname = script
        .find("'--hostname=$(touch /tmp/host)'")
        .unwrap_or_else(|| unreachable!("extra hostname"));
    let owned_login = script
        .find("'--login-server=https://headscale.example/'")
        .unwrap_or_else(|| unreachable!("owned login"));
    let owned_hostname = script
        .find("'--hostname=ci-guest'")
        .unwrap_or_else(|| unreachable!("owned hostname"));
    let auth_key = script
        .find("\"--auth-key=file:$key_path\"")
        .unwrap_or_else(|| unreachable!("auth key"));
    assert!(extra_hostname < owned_login);
    assert!(owned_login < owned_hostname);
    assert!(owned_hostname < auth_key);
    let logout = script
        .find("tailscale' logout")
        .unwrap_or_else(|| unreachable!("logout"));
    let login = script
        .find("tailscale' up")
        .unwrap_or_else(|| unreachable!("login"));
    assert!(logout < login);
}

#[test]
fn login_reasserts_the_configured_operator_before_the_status_check() {
    let script = tailscale_script(
        TART_TAILSCALE_PATH,
        "https://headscale.example/",
        None,
        &[],
        "builder",
    );
    let login = script
        .find("tailscale' up")
        .unwrap_or_else(|| unreachable!("login"));
    let operator = script
        .find("'--operator=builder'")
        .unwrap_or_else(|| unreachable!("operator"));
    let status = script
        .find("tailscale' status")
        .unwrap_or_else(|| unreachable!("status"));
    assert!(login < operator);
    assert!(operator < status);
}

#[tokio::test]
async fn disabled_bootstrap_does_not_read_a_key_or_start_ssh() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*flanforge_test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.ssh_path = "/path/that/does/not/exist".into();
    let guest = GuestControl::new(
        config.guest,
        config.tailscale,
        config.runtime.ssh_path,
        config.runtime.scp_path,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    guest
        .ensure_tailscale_connected(&session())
        .await
        .unwrap_or_else(|error| unreachable!("disabled: {error}"));
}

#[tokio::test]
async fn key_is_delivered_over_stdin_and_never_put_in_the_ssh_command() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let key_path = directory.path().join("headscale-key");
    let key = "headscale-preauth-secret-value";
    std::fs::write(&key_path, format!("{key}\n"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let arguments_path = directory.path().join("arguments");
    let input_path = directory.path().join("input");
    let source = format!(
        "printf '%s\\n' \"$*\" > {}\nIFS= read -r key\nprintf '%s' \"$key\" > {}\n",
        shell_quote(&arguments_path.to_string_lossy()),
        shell_quote(&input_path.to_string_lossy()),
    );
    let ssh_path = flanforge_test_support::executable(directory.path(), "ssh", &source);

    let mut config = (*flanforge_test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.ssh_path = ssh_path;
    config.tailscale.enabled = true;
    config.tailscale.preauth_key_file = Some(key_path);
    config.tailscale.login_server = Some(
        Url::parse("https://headscale.example")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    config.tailscale.hostname = Some("ci-guest".into());
    config.tailscale.extra_args = "--accept-dns=false --advertise-tags='tag:ci builder'".into();
    config
        .ensure_valid()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let guest = GuestControl::new(
        config.guest,
        config.tailscale,
        config.runtime.ssh_path,
        config.runtime.scp_path,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    guest
        .ensure_tailscale_connected(&session())
        .await
        .unwrap_or_else(|error| unreachable!("bootstrap: {error}"));

    let arguments = std::fs::read_to_string(arguments_path)
        .unwrap_or_else(|error| unreachable!("arguments: {error}"));
    assert!(!arguments.contains(key));
    assert!(arguments.contains("--auth-key=file:$key_path"));
    assert!(arguments.contains("'--advertise-tags=tag:ci builder'"));
    assert!(arguments.contains("'--operator=runner'"));
    let delivered =
        std::fs::read_to_string(input_path).unwrap_or_else(|error| unreachable!("input: {error}"));
    assert_eq!(delivered, key);
}

#[tokio::test]
async fn group_readable_key_file_is_rejected_before_ssh() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let key_path = directory.path().join("headscale-key");
    std::fs::write(&key_path, "headscale-preauth-secret-value")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o640))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*flanforge_test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.ssh_path = "/path/that/does/not/exist".into();
    config.tailscale.enabled = true;
    config.tailscale.preauth_key_file = Some(key_path);
    config.tailscale.login_server = Some(
        Url::parse("https://headscale.example")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let guest = GuestControl::new(
        config.guest,
        config.tailscale,
        config.runtime.ssh_path,
        config.runtime.scp_path,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(guest.ensure_tailscale_connected(&session()).await.is_err());
}
