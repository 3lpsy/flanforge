use std::{io::Write, os::unix::fs::PermissionsExt, sync::Arc};

use flanforge_core::{
    Allocation, AllocationMode, Config, Profile, RequestOptions, RunnerLabel, VmName,
};
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::{ForgejoClient, ForgejoError, RunnerCredentials};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationWorker, ConfigHandle, WorkerError,
};
use flanforge_store::{AllocationStore, JsonStateStore, JsonWarmImageStore, WarmImageStore};
use flanforge_test_support as test_support;

use super::{
    FlanForgeWorker, ensure_guest_known_hosts,
    guest::{
        GuestControl, REGENERATION_SENTINEL, runner_script_with_token_path, shell_quote,
        strip_script,
    },
    supervisor::SupervisionWindow,
    tart::TartClient,
};

#[test]
fn shell_quote_handles_single_quotes_without_evaluation() {
    assert_eq!(shell_quote("a'b;$HOME"), "'a'\\''b;$HOME'");
}

#[test]
fn shell_quote_handles_empty_strings() {
    assert_eq!(shell_quote(""), "''");
}

/// Hardening the guest channel must not depend on host-key verification.
const HARDENING_OPTIONS: [&str; 9] = [
    "GlobalKnownHostsFile=/dev/null",
    "BatchMode=yes",
    "IdentitiesOnly=yes",
    "ForwardAgent=no",
    "ForwardX11=no",
    "ControlMaster=no",
    "ControlPath=none",
    "PermitLocalCommand=no",
    "ProxyCommand=none",
];

fn guest_arguments(config: &flanforge_core::Config) -> Vec<String> {
    GuestControl::new(
        config.guest.clone(),
        config.tailscale.clone(),
        config.runtime.ssh_path.clone(),
        config.runtime.scp_path.clone(),
    )
    .base_arguments()
}

fn assert_channel_is_hardened(arguments: &[String]) {
    assert_eq!(arguments.first().map(String::as_str), Some("-F"));
    assert_eq!(arguments.get(1).map(String::as_str), Some("/dev/null"));
    for option in HARDENING_OPTIONS {
        assert!(arguments.iter().any(|value| value == option), "{option}");
    }
    assert!(arguments.iter().any(|value| value == "-i"));
    assert!(arguments.iter().any(|value| value == "/private/id"));
}

#[test]
fn guest_connections_pin_identity_and_host_keys() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let arguments = guest_arguments(&config);
    assert_channel_is_hardened(&arguments);
    assert!(
        arguments
            .iter()
            .any(|value| value == "StrictHostKeyChecking=yes")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "UserKnownHostsFile=\"/private/known_hosts\"")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "HostKeyAlias=flanforge-guest")
    );
    assert!(!arguments.iter().any(|value| value.contains("accept-new")));
}

#[test]
fn disabled_verification_drops_only_the_host_key_pinning() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.guest.verify_host_key = false;
    let arguments = guest_arguments(&config);
    assert_channel_is_hardened(&arguments);
    assert!(
        arguments
            .iter()
            .any(|value| value == "StrictHostKeyChecking=no")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "UserKnownHostsFile=/dev/null")
    );
    assert!(!arguments.iter().any(|value| value.contains("HostKeyAlias")));
    assert!(
        !arguments
            .iter()
            .any(|value| value.contains("/private/known_hosts"))
    );
}

#[test]
fn an_unset_anchor_still_fails_closed_while_verification_is_enabled() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.guest.ssh_known_hosts_file = None;
    config.guest.ssh_host_key_alias = None;
    let arguments = guest_arguments(&config);
    assert!(
        arguments
            .iter()
            .any(|value| value == "StrictHostKeyChecking=yes")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "UserKnownHostsFile=\"/dev/null\"")
    );
}

#[test]
fn known_hosts_path_with_spaces_stays_one_ssh_config_value() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.guest.ssh_known_hosts_file = Some("/Users/x/Application Support/known_hosts".into());
    assert!(
        guest_arguments(&config).iter().any(|value| value
            == "UserKnownHostsFile=\"/Users/x/Application Support/known_hosts\"")
    );
}

#[tokio::test]
async fn startup_refuses_a_known_hosts_file_without_the_configured_alias() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("known_hosts");
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.guest.ssh_known_hosts_file = Some(path.clone());

    assert!(ensure_guest_known_hosts(&config.guest).await.is_err());
    for contents in [
        "",
        "# flanforge-guest ssh-ed25519 AAAA\n",
        "other-host ssh-ed25519 AAAA\n",
        "flanforge-guest\n",
    ] {
        std::fs::write(&path, contents).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        assert!(
            ensure_guest_known_hosts(&config.guest).await.is_err(),
            "{contents:?}"
        );
    }

    for contents in [
        "flanforge-guest ssh-ed25519 AAAAC3NzaC1lZDI1NTE5\n",
        "@cert-authority flanforge-guest,192.0.2.10 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5\n",
    ] {
        std::fs::write(&path, contents).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        ensure_guest_known_hosts(&config.guest)
            .await
            .unwrap_or_else(|error| unreachable!("anchor {contents:?}: {error}"));
    }

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(ensure_guest_known_hosts(&config.guest).await.is_err());
}

#[tokio::test]
async fn startup_skips_the_anchor_check_only_when_verification_is_disabled() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.guest.ssh_known_hosts_file = Some(directory.path().join("missing_known_hosts"));
    assert!(ensure_guest_known_hosts(&config.guest).await.is_err());

    config.guest.verify_host_key = false;
    for anchor in [Some(directory.path().join("missing_known_hosts")), None] {
        config.guest.ssh_known_hosts_file = anchor;
        ensure_guest_known_hosts(&config.guest)
            .await
            .unwrap_or_else(|error| unreachable!("disabled verification: {error}"));
    }

    config.guest.verify_host_key = true;
    assert!(ensure_guest_known_hosts(&config.guest).await.is_err());
}

#[tokio::test]
async fn guest_commands_reject_unstructured_workflow_values_before_spawn() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let guest = GuestControl::new(
        config.guest.clone(),
        config.tailscale.clone(),
        config.runtime.ssh_path.clone(),
        config.runtime.scp_path.clone(),
    );
    let credentials = RunnerCredentials {
        id: 73,
        uuid: "392c9434-6bb9-454b-b9ff-646875cf6691".into(),
        token: "09d130cf90f9d757d83e5cc5a5338c470f04b71c".into(),
    };

    assert!(
        guest
            .spawn_runner(
                "127.0.0.1",
                "https://git.example",
                &credentials,
                "macos;touch-host",
                "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
            )
            .await
            .is_err()
    );
    assert!(
        guest
            .wait_ready("127.0.0.1;touch", std::time::Duration::from_secs(1))
            .await
            .is_err()
    );

    let invalid_credentials = RunnerCredentials {
        id: 73,
        uuid: "$(touch /tmp/host)".into(),
        token: credentials.token.clone(),
    };
    for (server_url, credentials, label, handle) in [
        (
            "https://git.example;touch",
            &credentials,
            "macos-allocation",
            "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
        ),
        (
            "https://git.example",
            &invalid_credentials,
            "macos-allocation",
            "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
        ),
        (
            "https://git.example",
            &credentials,
            "macos-allocation",
            "$(touch /tmp/host)",
        ),
    ] {
        assert!(
            guest
                .spawn_runner("127.0.0.1", server_url, credentials, label, handle)
                .await
                .is_err()
        );
    }
}

#[test]
fn runner_script_removes_the_token_after_runner_exit() {
    use std::process::{Command, Stdio};

    for exit_code in [0, 7] {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let runner = directory.path().join("runner");
        let arguments = directory.path().join("arguments");
        let runner_source = format!(
            "#!/bin/sh\n/usr/bin/printf '%s' \"$*\" > {}\nexit {exit_code}\n",
            shell_quote(&arguments.to_string_lossy())
        );
        std::fs::write(&runner, runner_source)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let token_path = directory.path().join("one-job-token");
        let script = runner_script_with_token_path(
            &runner.to_string_lossy(),
            "https://git.example",
            "uuid",
            "macos:host",
            "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
            &token_path.to_string_lossy(),
        );
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| unreachable!("shell: {error}"));
        child
            .stdin
            .take()
            .unwrap_or_else(|| unreachable!())
            .write_all(b"secret\n")
            .unwrap_or_else(|error| unreachable!("stdin: {error}"));
        let status = child
            .wait()
            .unwrap_or_else(|error| unreachable!("wait: {error}"));
        assert_eq!(status.code(), Some(exit_code));
        assert!(!token_path.exists());
        let arguments = std::fs::read_to_string(arguments)
            .unwrap_or_else(|error| unreachable!("arguments: {error}"));
        assert!(arguments.contains("--handle 33ba7d51-59c6-44f8-9d2b-1b94f4033973"));
    }
}

#[tokio::test]
async fn preexisting_prefixed_vm_is_deleted_before_clone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"ci-project-42-1","State":"stopped"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime.clone());
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    tart.ensure_can_clone(&allocation, &profile)
        .await
        .unwrap_or_else(|error| unreachable!("prepare clone: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete ci-project-42-1\n");
}

#[tokio::test]
async fn prefix_is_required_before_collision_deletion() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"personal-project-42-1","State":"stopped"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime.clone());
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("personal-project-42-1")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());

    assert!(tart.ensure_can_clone(&allocation, &profile).await.is_err());
    assert!(!log.exists(), "an unprefixed VM was modified");
}

#[tokio::test]
async fn cleanup_deletes_prefixed_vm_without_creation_marker() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"ci-project-42-1","State":"running"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime);
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );

    tart.remove_owned(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("cleanup: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "stop ci-project-42-1\ndelete ci-project-42-1\n");
}

#[tokio::test]
async fn failed_stop_still_deletes_the_clone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart_failing(
        &directory,
        r#"[{"Name":"ci-project-42-1","State":"running"}]"#,
        "stop",
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime);
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );

    tart.remove_owned(&allocation)
        .await
        .unwrap_or_else(|error| unreachable!("cleanup: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert!(
        mutations
            .lines()
            .any(|line| line == "delete ci-project-42-1")
    );
}

#[tokio::test]
async fn failed_delete_of_a_present_clone_is_an_error() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _) = fake_tart_failing(
        &directory,
        r#"[{"Name":"ci-project-42-1","State":"running"}]"#,
        "delete",
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime);
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );

    assert!(tart.remove_owned(&allocation).await.is_err());
}

#[tokio::test]
async fn allocation_subprocess_obeys_the_absolute_phase_deadline() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let script = directory.path().join("tart");
    std::fs::write(&script, "#!/bin/sh\nexec /bin/sleep 10\n")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime.clone());
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let started = tokio::time::Instant::now();

    let result = FlanForgeWorker::phase(
        &CancellationToken::new(),
        started + std::time::Duration::from_millis(50),
        "Tart preparation exceeded its timeout",
        tart.ensure_can_clone(&allocation, &profile),
    )
    .await;

    assert!(result.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn template_must_be_exactly_stopped() {
    for state in ["running", "suspended", "Stopped", "unknown"] {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let machines = format!(r#"[{{"Name":"flanforge-base","State":"{state}"}}]"#);
        let (script, log) = fake_tart(&directory, &machines);
        let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
        config.runtime.tart_path = script;
        let tart = TartClient::new(config.runtime.clone());
        let allocation = Allocation::new(
            test_support::request(),
            VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
            RunnerLabel::new("macos-tart-project-allocation")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            AllocationMode::Cold,
            test_support::size(),
        );
        let profile = config
            .profiles
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| unreachable!());

        assert!(tart.ensure_can_clone(&allocation, &profile).await.is_err());
        assert!(!log.exists(), "template state {state} caused a mutation");
    }
}

#[tokio::test]
async fn forgejo_timeout_does_not_prevent_vm_teardown() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"ci-project-42-1","State":"running"}]"#,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let application = axum::Router::new().fallback(|| async {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        axum::http::StatusCode::NO_CONTENT
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });

    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    config.forgejo.api_url = url::Url::parse(&format!("http://{address}/api/v1/"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let profile = config.profiles.values_mut().next().map_or_else(
        || unreachable!(),
        |profile| {
            profile.cleanup_timeout_seconds = 1;
            profile.clone()
        },
    );
    let forgejo = ForgejoClient::new(
        Arc::new(config.forgejo.clone()),
        "12345678901234567890".into(),
    )
    .unwrap_or_else(|error| unreachable!("client: {error}"));
    let worker = FlanForgeWorker::new(&config, forgejo);
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    allocation.set_vm_created();
    allocation.set_runner_id(73);
    assert!(worker.cleanup(allocation, profile).await.is_err());
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert!(mutations.lines().any(|line| line == "stop ci-project-42-1"));
    assert!(
        mutations
            .lines()
            .any(|line| line == "delete ci-project-42-1")
    );
}

#[tokio::test(start_paused = true)]
async fn unobservable_runner_status_is_bounded_by_the_job_ttl_not_the_idle_ttl() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let idle = std::time::Duration::from_secs(profile.idle_timeout_seconds);
    assert!(profile.job_timeout_seconds > profile.idle_timeout_seconds);

    let mut window = SupervisionWindow::new(&profile);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 2);
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner exceeded its supervised lifetime")
    );

    let mut window = SupervisionWindow::new(&profile);
    window.observed();
    tokio::time::advance(idle).await;
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner did not receive a job before its idle timeout")
    );

    // Accepting a job latches, so a later non-zero runner exit is judged
    // against a job that ran rather than one that never started.
    let mut window = SupervisionWindow::new(&profile);
    assert!(!window.is_running());
    window.start_job();
    assert!(window.is_running());
}

#[tokio::test(start_paused = true)]
async fn a_rejected_observation_does_not_hold_the_idle_window_open() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let idle = std::time::Duration::from_secs(profile.idle_timeout_seconds);

    // A rejected observation never becomes an accepted job, so an allocation
    // that can never be filled falls out through its idle timeout.
    let mut window = SupervisionWindow::new(&profile);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Api), 1);
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner did not receive a job before its idle timeout")
    );

    // An unreachable Forgejo still holds it open: that is not an idle runner.
    let mut window = SupervisionWindow::new(&profile);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);

    // Nor is a response this client cannot read: a renamed status or a changed
    // payload must not kill a runner that is working through its job.
    let mut window = SupervisionWindow::new(&profile);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Malformed), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);
}

#[tokio::test]
async fn an_absent_registration_lets_a_running_job_finish_its_shutdown() {
    // Forgejo reports the job as accepted once, then deletes the registration
    // the way a cancelled run does.
    let fixture = SupervisedFixture::new(1).await;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "sleep 4; exit 1"])
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let started = tokio::time::Instant::now();

    // A runner that exits inside the grace window completed its job, whatever
    // its exit code says, so the allocation still succeeds.
    let result = fixture.supervise(child).await;

    assert!(result.is_ok(), "supervise: {result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    // A deleted registration is polled once and never again.
    assert!(fixture.polls_at_first_absent() > 0);
    assert_eq!(fixture.polls(), fixture.polls_at_first_absent());
}

/// RUN-600: a job short enough to end between two polls is never sampled as
/// active, so an absent registration is no evidence that no job ever ran.
#[tokio::test]
async fn a_job_that_ends_before_any_poll_observes_it_still_succeeds() {
    let fixture = SupervisedFixture::new(0).await;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "sleep 3; exit 0"])
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // The runner's own exit still decides the allocation, so retention runs
    // instead of the run being reported as an infrastructure failure.
    let result = fixture.supervise(child).await;

    assert!(result.is_ok(), "supervise: {result:?}");
    assert!(fixture.polls_at_first_absent() > 0);
    assert_eq!(fixture.polls(), fixture.polls_at_first_absent());
}

#[tokio::test]
async fn a_released_runner_that_never_exits_is_killed_within_its_supervised_lifetime() {
    let fixture = SupervisedFixture::new(0).await.with_job_timeout(5);
    let mut child = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut stdout = child
        .stdout
        .take()
        .unwrap_or_else(|| unreachable!("fixture: the child has no piped stdout"));
    let started = tokio::time::Instant::now();

    let result = fixture.supervise(child).await;

    assert_eq!(
        result.map_err(|error| error.to_string()),
        Err("guest runner did not exit after Forgejo released its registration".to_owned())
    );
    // A runner that never shuts down must not wait out the idle timeout, let
    // alone the job timeout the bug reached.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(fixture.profile.idle_timeout_seconds)
    );
    // The guest runner's pipe only closes once its process is gone, so the
    // slot is released rather than held by a killed-but-living child.
    let mut byte = [0_u8; 1];
    assert!(matches!(
        tokio::io::AsyncReadExt::read(&mut stdout, &mut byte).await,
        Ok(0)
    ));
}

/// One exit rule serves both the observed and the released path, so a runner
/// that never reached a job is judged the same either way.
#[tokio::test]
async fn a_released_runner_that_never_took_a_job_fails_on_an_unsuccessful_exit() {
    let fixture = SupervisedFixture::new(0).await;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "exit 3"])
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let result = fixture.supervise(child).await;

    assert_eq!(
        result.map_err(|error| error.to_string()),
        Err("guest runner exited before completing a job".to_owned())
    );
}

#[tokio::test]
async fn a_transient_job_poll_failure_is_retried_but_a_mismatch_is_not() {
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let application = axum::Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            axum::routing::get(
                |axum::extract::State(requests): axum::extract::State<
                    Arc<std::sync::atomic::AtomicUsize>,
                >| async move {
                    if requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                    }
                    Ok(axum::Json(serde_json::json!([{
                        "attempt": 1,
                        "handle": "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
                        "name": "apple-build",
                        "runs_on": ["macos-tart-project-allocation"],
                        "status": "waiting"
                    }])))
                },
            ),
        )
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.forgejo.api_url = url::Url::parse(&format!("http://{address}/api/v1/"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let forgejo = ForgejoClient::new(
        Arc::new(config.forgejo.clone()),
        "12345678901234567890".into(),
    )
    .unwrap_or_else(|error| unreachable!("client: {error}"));
    let worker = FlanForgeWorker::new(&config, forgejo);
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

    let handle = worker
        .wait_for_job_handle(&allocation, &profile, &CancellationToken::new(), deadline)
        .await
        .unwrap_or_else(|error| unreachable!("poll: {error}"));
    assert_eq!(handle, "33ba7d51-59c6-44f8-9d2b-1b94f4033973");
    assert!(requests.load(std::sync::atomic::Ordering::SeqCst) >= 2);

    profile.job_name = "another-build".into();
    assert!(
        worker
            .wait_for_job_handle(&allocation, &profile, &CancellationToken::new(), deadline)
            .await
            .is_err()
    );
}

/// One supervisable allocation whose Forgejo reports an accepted job for the
/// first `active_polls` status reads and a deleted registration after that.
struct SupervisedFixture {
    worker: FlanForgeWorker,
    allocation: Allocation,
    profile: Profile,
    reporter: AllocationReporter,
    polls: Arc<std::sync::atomic::AtomicUsize>,
    first_absent: Arc<std::sync::atomic::AtomicUsize>,
    _manager: AllocationManager,
    _directory: tempfile::TempDir,
}

impl SupervisedFixture {
    async fn new(active_polls: usize) -> Self {
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_absent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&polls);
        let absent = Arc::clone(&first_absent);
        let application = axum::Router::new().route(
            "/api/v1/repos/owner/project/actions/runners/{id}",
            axum::routing::get(move || {
                let polls = Arc::clone(&counter);
                let first_absent = Arc::clone(&absent);
                async move {
                    let poll = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if poll <= active_polls {
                        return Ok(axum::Json(
                            serde_json::json!({"id": 73, "status": "active"}),
                        ));
                    }
                    // Remember how far polling had got when the registration
                    // first read as deleted, so a test can assert it stopped.
                    let _ = first_absent.compare_exchange(
                        0,
                        poll,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    Err(axum::http::StatusCode::NOT_FOUND)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, application).await;
        });

        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
        config.forgejo.api_url = url::Url::parse(&format!("http://{address}/api/v1/"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let config = Arc::new(config);
        let profile = config
            .profiles
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| unreachable!());
        let forgejo = ForgejoClient::new(
            Arc::new(config.forgejo.clone()),
            "12345678901234567890".into(),
        )
        .unwrap_or_else(|error| unreachable!("client: {error}"));
        let (manager, mut allocation, reporter) = waiting_allocation(&config, &directory).await;
        allocation.set_runner_id(73);
        Self {
            worker: FlanForgeWorker::new(&config, forgejo),
            allocation,
            profile,
            reporter,
            polls,
            first_absent,
            _manager: manager,
            _directory: directory,
        }
    }

    /// Shortens the supervised lifetime, which also caps how long a released
    /// runner may keep shutting down.
    fn with_job_timeout(mut self, seconds: u64) -> Self {
        self.profile.job_timeout_seconds = seconds;
        self
    }

    fn polls(&self) -> usize {
        self.polls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many polls had run when Forgejo first reported the runner absent.
    fn polls_at_first_absent(&self) -> usize {
        self.first_absent.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn supervise(&self, child: tokio::process::Child) -> Result<(), WorkerError> {
        self.worker
            .supervise(
                child,
                &self.allocation,
                &self.profile,
                &self.reporter,
                &CancellationToken::new(),
            )
            .await
    }
}

/// A worker that reports its way to `WaitingForJob` and hands the reporter
/// back, since only the manager can mint one.
#[derive(Debug)]
struct ReportingWorker(tokio::sync::mpsc::UnboundedSender<AllocationReporter>);

#[async_trait::async_trait]
impl AllocationWorker for ReportingWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        for state in [
            flanforge_core::AllocationState::Preparing,
            flanforge_core::AllocationState::Booting,
            flanforge_core::AllocationState::Registering,
            flanforge_core::AllocationState::WaitingForJob,
        ] {
            reporter
                .transition(state)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
        }
        let _ = self.0.send(reporter);
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn cleanup(&self, _allocation: Allocation, _profile: Profile) -> Result<(), WorkerError> {
        Ok(())
    }
}

async fn waiting_allocation(
    config: &Arc<Config>,
    directory: &tempfile::TempDir,
) -> (AllocationManager, Allocation, AllocationReporter) {
    let store: Arc<dyn AllocationStore> = Arc::new(
        JsonStateStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        JsonWarmImageStore::open(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(config)),
        store,
        images,
        Arc::new(ReportingWorker(sender)),
    );
    let allocation = manager
        .create(
            test_support::request(),
            RequestOptions::default(),
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"))
        .allocation;
    let reporter = receiver
        .recv()
        .await
        .unwrap_or_else(|| unreachable!("fixture: the worker reported no reporter"));
    (manager, allocation, reporter)
}

pub(crate) fn fake_tart(
    directory: &tempfile::TempDir,
    machines: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    fake_tart_failing(directory, machines, "")
}

pub(crate) fn fake_tart_failing(
    directory: &tempfile::TempDir,
    machines: &str,
    failing: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let script = directory.path().join("tart");
    let log = directory.path().join("mutations.log");
    let source = format!(
        "#!/bin/sh\nif [ \"$1\" = list ]; then\n  /usr/bin/printf '%s' {}\nelse\n  /usr/bin/printf '%s\\n' \"$*\" >> {}\n  [ \"$1\" != {} ] || exit 1\nfi\n",
        shell_quote(machines),
        shell_quote(&log.to_string_lossy()),
        shell_quote(failing),
    );
    std::fs::write(&script, source).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    (script, log)
}

/// The strip is a fixed script: no workflow value can reach the guest shell.
#[test]
fn the_strip_script_interpolates_no_workflow_data() {
    let script = strip_script();
    for value in [
        "owner/project",
        "refs/heads/main",
        "apple.yml",
        "`",
        "$(",
        "${",
    ] {
        assert!(!script.contains(value), "{value}");
    }
    // Only the guest's own shell variables are expanded; every path is a
    // single-quoted literal the daemon wrote.
    let expanded = script
        .replace("$HOME", "")
        .replace("$sentinel", "")
        .replace("$relative", "")
        .replace("$absolute", "");
    assert!(!expanded.contains('$'), "{expanded}");
}

#[test]
fn the_strip_script_removes_the_tailnet_identity_and_keeps_the_host_keys() {
    let script = strip_script();
    assert!(script.contains("tailscale' logout"));
    assert!(script.contains("/opt/homebrew/var/lib/tailscale"));
    assert!(script.contains(".ssh/known_hosts"));
    for kept in [
        "authorized_keys",
        "ssh_host",
        "/etc/ssh",
        "/Users/runner/bin/forgejo-runner",
    ] {
        assert!(!script.contains(kept), "{kept}");
    }
}

fn staged_guest_home(directory: &tempfile::TempDir) -> std::path::PathBuf {
    let home = directory.path().join("runner");
    for relative in [".ssh", ".config/git", ".cache/act", "_work"] {
        std::fs::create_dir_all(home.join(relative))
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    for relative in [
        ".gitconfig",
        ".git-credentials",
        ".netrc",
        ".ssh/known_hosts",
        ".bash_history",
        ".ssh/authorized_keys",
    ] {
        std::fs::write(home.join(relative), b"x")
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    home
}

fn run_strip(home: &std::path::Path) -> Option<i32> {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(strip_script())
        .env("HOME", home)
        .status()
        .unwrap_or_else(|error| unreachable!("strip: {error}"))
        .code()
}

#[test]
fn a_missing_sentinel_rejects_retention() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let home = staged_guest_home(&directory);

    assert_eq!(run_strip(&home), Some(10));
    assert!(home.join(".git-credentials").exists());
}

#[test]
fn the_strip_removes_every_credential_class_and_keeps_the_boot_path() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let home = staged_guest_home(&directory);
    std::fs::write(home.join(REGENERATION_SENTINEL), b"ok")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(run_strip(&home), Some(0));
    for removed in [
        ".gitconfig",
        ".git-credentials",
        ".netrc",
        ".ssh/known_hosts",
        ".bash_history",
        ".config/git",
        ".cache/act",
        "_work",
        REGENERATION_SENTINEL,
    ] {
        assert!(!home.join(removed).exists(), "{removed}");
    }
    assert!(home.join(".ssh/authorized_keys").exists());
}

#[tokio::test]
async fn a_failed_strip_aborts_retention() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.ssh_path = "/bin/false".into();
    let guest = GuestControl::new(
        config.guest.clone(),
        config.tailscale.clone(),
        config.runtime.ssh_path.clone(),
        config.runtime.scp_path.clone(),
    );

    assert!(guest.ensure_stripped("192.0.2.10").await.is_err());
    assert!(guest.ensure_stripped("not-an-ip").await.is_err());
}

#[tokio::test]
async fn ensure_image_writable_rejects_the_prefix_and_the_profile_template() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::warm_config(directory.path().to_path_buf());
    let tart = TartClient::new(config.runtime.clone());
    let profile = test_support::warm_profile();

    for permitted in [
        "project-warm",
        "project-warm.previous",
        "project-warm.staging",
    ] {
        let name = VmName::new(permitted).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        assert!(
            tart.ensure_image_writable(&name, Some(&profile), None)
                .is_ok(),
            "{permitted}"
        );
    }
    for refused in ["ci-project-42-1", "flanforge-base", "someone-elses-vm"] {
        let name = VmName::new(refused).unwrap_or_else(|error| unreachable!("fixture: {error}"));
        assert!(
            tart.ensure_image_writable(&name, Some(&profile), None)
                .is_err(),
            "{refused}"
        );
    }
}

#[tokio::test]
async fn a_warm_source_deleted_between_selection_and_clone_falls_back() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime.clone());
    let profile = test_support::warm_profile();
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Warm,
        test_support::size(),
    );
    allocation.set_source(flanforge_core::CloneSource {
        name: VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        kind: flanforge_core::CloneKind::Warm,
        base_fingerprint: None,
        fallback_reason: None,
    });

    let source = tart
        .ensure_can_clone(&allocation, &profile)
        .await
        .unwrap_or_else(|error| unreachable!("prepare clone: {error}"));
    assert_eq!(source.name, profile.template);
    assert_eq!(
        source.fallback_reason,
        Some(flanforge_core::FallbackReason::Absent)
    );
}

#[tokio::test]
async fn a_warm_source_inside_the_service_prefix_is_refused() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"ci-sneaky","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let tart = TartClient::new(config.runtime.clone());
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Warm,
        test_support::size(),
    );
    allocation.set_source(flanforge_core::CloneSource {
        name: VmName::new("ci-sneaky").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        kind: flanforge_core::CloneKind::Warm,
        base_fingerprint: None,
        fallback_reason: None,
    });

    let source = tart
        .ensure_can_clone(&allocation, &test_support::warm_profile())
        .await
        .unwrap_or_else(|error| unreachable!("prepare clone: {error}"));
    assert_eq!(source.name, test_support::warm_profile().template);
}

#[tokio::test]
async fn a_rebuilt_base_changes_its_fingerprint_and_an_unreadable_one_has_none() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_home = Some(directory.path().to_path_buf());
    let base = directory.path().join("vms").join("flanforge-base");
    std::fs::create_dir_all(&base).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let tart = TartClient::new(config.runtime.clone());
    let template =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(tart.fingerprint(&template).await, None);
    std::fs::write(base.join("disk.img"), b"one")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::write(base.join("config.json"), b"{}")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let first = tart.fingerprint(&template).await;
    assert!(first.is_some());

    std::fs::write(base.join("disk.img"), b"rebuilt")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_ne!(tart.fingerprint(&template).await, first);
}

#[tokio::test]
async fn an_undeterminable_age_is_reported_as_unknown() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    config.runtime.tart_home = None;
    let machines = TartClient::new(config.runtime)
        .host_machines()
        .await
        .unwrap_or_else(|error| unreachable!("machines: {error}"));

    assert_eq!(machines.len(), 1);
    assert_eq!(
        machines.first().and_then(|machine| machine.age_seconds),
        None
    );
}

#[tokio::test]
async fn the_runtime_refuses_a_deletion_no_record_authorizes() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"stopped"},{"Name":"flanforge-base","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let worker = FlanForgeWorker::new(
        &config,
        ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let profile = test_support::warm_profile();
    let reserved = flanforge_manager::reserved_image_names(&config);
    let orphan =
        VmName::new("ci-project-41-1").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let template =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // A prefix-shaped name with no record is never deletable, whatever asks.
    assert!(
        worker
            .delete_vm(flanforge_manager::ReapRequest {
                name: &orphan,
                authorization: &flanforge_manager::ReapAuthorization::Prefix,
                profile: None,
                record: None,
                reserved: &reserved,
            })
            .await
            .is_err()
    );
    // Image authority never reaches a configured template.
    assert!(
        worker
            .delete_vm(flanforge_manager::ReapRequest {
                name: &template,
                authorization: &flanforge_manager::ReapAuthorization::Image(
                    test_support::profile_name()
                ),
                profile: Some(&profile),
                record: None,
                reserved: &reserved,
            })
            .await
            .is_err()
    );
    assert!(!log.exists(), "an unauthorized deletion reached Tart");

    worker
        .delete_vm(flanforge_manager::ReapRequest {
            name: &orphan,
            authorization: &flanforge_manager::ReapAuthorization::Record(
                flanforge_core::AllocationId::new(),
            ),
            profile: None,
            record: None,
            reserved: &reserved,
        })
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete ci-project-41-1\n");
}

/// RUN-540: a record whose profile was removed is still the daemon's own claim
/// on those names, and live configuration is still off limits.
#[tokio::test]
async fn a_record_authorizes_a_deletion_after_its_profile_is_removed() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"project-retired","State":"stopped"},{"Name":"flanforge-base","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    config.runtime.tart_path = script;
    let worker = FlanForgeWorker::new(
        &config,
        ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let reserved = flanforge_manager::reserved_image_names(&config);
    let retired =
        VmName::new("project-retired").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let record = flanforge_core::WarmImageRecord {
        profile: test_support::profile_name(),
        warm_template: retired.clone(),
        generation: 2,
        base_fingerprint: flanforge_core::BaseFingerprint::new("aa01")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        produced_by: flanforge_core::AllocationId::new(),
        produced_at_unix: 1_755_324_251,
        state: flanforge_core::WarmImageState::Promoted,
        previous: None,
    };
    let authorization = flanforge_manager::ReapAuthorization::Image(test_support::profile_name());

    worker
        .delete_vm(flanforge_manager::ReapRequest {
            name: &retired,
            authorization: &authorization,
            profile: None,
            record: Some(&record),
            reserved: &reserved,
        })
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
    let mutations =
        std::fs::read_to_string(&log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete project-retired\n");

    // A record can never reach a name live configuration still claims.
    let claimed =
        VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let claiming = flanforge_core::WarmImageRecord {
        warm_template: claimed.clone(),
        ..record
    };
    assert!(
        worker
            .delete_vm(flanforge_manager::ReapRequest {
                name: &claimed,
                authorization: &authorization,
                profile: None,
                record: Some(&claiming),
                reserved: &reserved,
            })
            .await
            .is_err()
    );
    let mutations =
        std::fs::read_to_string(&log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete project-retired\n");
}
