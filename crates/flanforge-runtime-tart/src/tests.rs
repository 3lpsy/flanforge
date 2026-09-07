use std::{io::Write, os::unix::fs::PermissionsExt, sync::Arc};

use flanforge_core::{
    Allocation, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource, Config,
    Profile, RequestOptions, RunnerLabel, VmName, WarmGeneration, WarmImageRecord, WarmImageState,
};
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::{ForgejoClient, ForgejoError, RunnerCredentials, RunnerStatus};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
    MachineOwnership, WorkerError,
};
use flanforge_runtime::{
    GuestControl, SshChannel, StartedRunner, SupervisionWindow, ensure_guest_known_hosts,
    retention_marker_script, runner_script_with_token_template, shell_quote, ssh_process_for_test,
};
use flanforge_store::{AllocationStore, WarmImageStore};
use flanforge_test_support as test_support;

use flanforge_runtime::REGENERATION_SENTINEL;

use super::{
    FlanForgeWorker,
    retention::RetentionRequest,
    tart::{TartClient, disk_size_argument, library_home},
};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

const GIB: u64 = 1_024 * 1_024 * 1_024;

/// Ample slack for the tests that are not about the capacity wait itself.
fn clone_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_secs(30)
}

fn tart_config(config: &mut Config) -> &mut flanforge_core::TartConfig {
    config
        .runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
}

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
    SshChannel::new(
        &config.guest,
        config.runtime.ssh_path.clone(),
        config.runtime.scp_path.clone(),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
    .base_arguments()
}

fn guest_ssh(config: &mut flanforge_core::Config) -> &mut flanforge_core::GuestSshConfig {
    config
        .guest
        .ssh
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"))
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
    guest_ssh(&mut config).verify_host_key = false;
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
    guest_ssh(&mut config).known_hosts_file = None;
    guest_ssh(&mut config).host_key_alias = None;
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
    guest_ssh(&mut config).known_hosts_file =
        Some("/Users/x/Application Support/known_hosts".into());
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
    guest_ssh(&mut config).known_hosts_file = Some(path.clone());

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
    guest_ssh(&mut config).known_hosts_file = Some(directory.path().join("missing_known_hosts"));
    assert!(ensure_guest_known_hosts(&config.guest).await.is_err());

    guest_ssh(&mut config).verify_host_key = false;
    for anchor in [Some(directory.path().join("missing_known_hosts")), None] {
        guest_ssh(&mut config).known_hosts_file = anchor;
        ensure_guest_known_hosts(&config.guest)
            .await
            .unwrap_or_else(|error| unreachable!("disabled verification: {error}"));
    }

    guest_ssh(&mut config).verify_host_key = true;
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
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let credentials = RunnerCredentials {
        id: 73,
        uuid: "392c9434-6bb9-454b-b9ff-646875cf6691".into(),
        token: "09d130cf90f9d757d83e5cc5a5338c470f04b71c".into(),
    };

    let session = flanforge_runtime::GuestSession::configured("127.0.0.1")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let spawn = |server_url, credentials, label, handle| flanforge_runtime::RunnerSpawn {
        server_url,
        credentials,
        label,
        handle,
        allocation_id: uuid::Uuid::new_v4(),
        lifetime: std::time::Duration::from_mins(5),
    };

    assert!(
        guest
            .spawn_session_runner(
                &session,
                &spawn(
                    "https://git.example",
                    &credentials,
                    "macos;touch-host",
                    "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
                ),
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
                .spawn_session_runner(&session, &spawn(server_url, credentials, label, handle))
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
        let arguments = directory.path().join("arguments");
        let token_mode = directory.path().join("token-mode");
        let runner_source = format!(
            "/usr/bin/printf '%s' \"$*\" > {}\ntoken_path=${{7#file://}}\n/bin/ls -ld \"$token_path\" | /usr/bin/cut -c1-10 > {}\nexit {exit_code}\n",
            shell_quote(&arguments.to_string_lossy()),
            shell_quote(&token_mode.to_string_lossy()),
        );
        let runner = test_support::executable(directory.path(), "runner", &runner_source);
        let token_template = directory.path().join("one-job-token.XXXXXX");
        let script = runner_script_with_token_template(
            &runner.to_string_lossy(),
            "https://git.example",
            "uuid",
            "macos:host",
            "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
            &token_template.to_string_lossy(),
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
        let arguments = std::fs::read_to_string(arguments)
            .unwrap_or_else(|error| unreachable!("arguments: {error}"));
        assert!(arguments.contains("--handle 33ba7d51-59c6-44f8-9d2b-1b94f4033973"));
        let token_path = arguments
            .split_whitespace()
            .find_map(|argument| argument.strip_prefix("file://"))
            .unwrap_or_else(|| unreachable!("token path"));
        assert!(!std::path::Path::new(token_path).exists());
        assert_eq!(
            std::fs::read_to_string(token_mode)
                .unwrap_or_else(|error| unreachable!("mode: {error}"))
                .trim(),
            "-rw-------"
        );
    }
}

#[test]
fn runner_script_refuses_an_unreplaceable_token_path_before_launch() {
    use std::process::{Command, Stdio};

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let marker = directory.path().join("runner-started");
    let runner = test_support::executable(
        directory.path(),
        "runner",
        &format!(
            "/usr/bin/touch {}\n",
            shell_quote(&marker.to_string_lossy())
        ),
    );
    let occupied = directory.path().join("not-a-directory");
    std::fs::write(&occupied, "occupied").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let invalid_template = occupied.join("token.XXXXXX");
    let script = runner_script_with_token_template(
        &runner.to_string_lossy(),
        "https://git.example",
        "uuid",
        "macos:host",
        "handle",
        &invalid_template.to_string_lossy(),
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
        .unwrap_or_else(|| unreachable!("stdin"))
        .write_all(b"secret\n")
        .unwrap_or_else(|error| unreachable!("stdin: {error}"));
    let status = child
        .wait()
        .unwrap_or_else(|error| unreachable!("wait: {error}"));
    assert!(!status.success());
    assert!(!marker.exists());
}

#[tokio::test]
async fn preexisting_prefixed_vm_is_deleted_before_clone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"ci-project-42-1","State":"stopped"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
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
    tart.ensure_can_clone(&allocation, &profile, clone_deadline())
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
    tart_config(&mut config).path = script;
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

    assert!(
        tart.ensure_can_clone(&allocation, &profile, clone_deadline())
            .await
            .is_err()
    );
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
    tart_config(&mut config).path = script;
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
    tart_config(&mut config).path = script;
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
    tart_config(&mut config).path = script;
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
    let script = test_support::executable(directory.path(), "tart", "exec /bin/sleep 10\n");
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
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
        tart.ensure_can_clone(
            &allocation,
            &profile,
            started + std::time::Duration::from_millis(50),
        ),
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
        tart_config(&mut config).path = script;
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

        assert!(
            tart.ensure_can_clone(&allocation, &profile, clone_deadline())
                .await
                .is_err()
        );
        assert!(!log.exists(), "template state {state} caused a mutation");
    }
}

/// ARCH-307: a slot taken between admission and the clone used to fail the
/// allocation outright. The clone now waits for the slot inside the phase.
#[tokio::test]
async fn a_slot_taken_before_the_clone_is_waited_for_rather_than_failed() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, calls) = fake_tart_freeing(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"devboxvm","State":"running"},{"Name":"otherbox","State":"running"}]"#,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"devboxvm","State":"running"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
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

    let source = tart
        .ensure_can_clone(&allocation, &profile, clone_deadline())
        .await
        .unwrap_or_else(|error| unreachable!("prepare clone: {error}"));

    assert_eq!(source.name, profile.template);
    let listed = std::fs::read_to_string(calls)
        .unwrap_or_else(|error| unreachable!("list calls: {error}"))
        .lines()
        .count();
    assert!(listed >= 2, "capacity was not re-listed: {listed} call(s)");
}

/// ARCH-307: exhaustion that outlives the wait is a capacity outcome, so the
/// route can answer busy instead of reporting a broken build.
#[tokio::test]
async fn exhausted_capacity_is_a_capacity_outcome_not_a_generic_failure() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"flanforge-base","State":"stopped"},{"Name":"devboxvm","State":"running"},{"Name":"otherbox","State":"running"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
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
    // Shorter than one poll interval, so the wait ends on its first pass and
    // the test never sleeps.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);

    let Err(error) = tart.ensure_can_clone(&allocation, &profile, deadline).await else {
        unreachable!("a full host must not admit a clone");
    };

    assert!(error.is_capacity(), "{error}");
    assert!(!log.exists(), "a full host caused a mutation");
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
    // RUN-601: a server that never answers makes the Forgejo timeout a
    // property rather than a race with a sleep the teardown budget has to
    // undercut.
    let application = axum::Router::new().fallback(std::future::pending::<axum::http::StatusCode>);
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });

    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
    config.forgejo.api_url = url::Url::parse(&format!("http://{address}/api/v1/"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    // The client's own budget ends the Forgejo call, so the cleanup budget no
    // longer has to be small enough to expire and large enough to tear down.
    config.forgejo.http_timeout_seconds = 1;
    let profile = config.profiles.values_mut().next().map_or_else(
        || unreachable!(),
        |profile| {
            profile.cleanup_timeout_seconds = 600;
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
    assert!(
        worker
            .cleanup(
                allocation,
                CleanupBudget::allow(std::time::Duration::from_secs(
                    profile.cleanup_timeout_seconds,
                )),
            )
            .await
            .is_err()
    );
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

    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 2);
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner exceeded its supervised lifetime")
    );

    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    window.observed();
    tokio::time::advance(idle).await;
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner did not receive a job before its idle timeout")
    );

    // Accepting a job latches, so a later non-zero runner exit is judged
    // against a job that ran rather than one that never started.
    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    assert!(!window.is_running());
    assert_eq!(
        window.advance(RunnerStatus::Active),
        [AllocationState::Ready, AllocationState::Running].as_slice()
    );
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
    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Api), 1);
    assert_eq!(
        window.overrun(tokio::time::Instant::now()),
        Some("guest runner did not receive a job before its idle timeout")
    );

    // An unreachable Forgejo still holds it open: that is not an idle runner.
    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Unavailable), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);

    // Nor is a response this client cannot read: a renamed status or a changed
    // payload must not kill a runner that is working through its job.
    let mut window = SupervisionWindow::new(&profile, tokio::time::Instant::now() + idle);
    tokio::time::advance(idle).await;
    assert_eq!(window.unobserved(ForgejoError::Malformed), 1);
    assert_eq!(window.overrun(tokio::time::Instant::now()), None);
}

/// RUN-247: waiting for the job and waiting for the runner to take it are one
/// idle budget, so an allocation that never runs a job cannot hold its slot for
/// two idle TTLs.
#[tokio::test(start_paused = true)]
async fn the_idle_budget_is_not_granted_twice() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let mut profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    profile.idle_timeout_seconds = 600;
    // The supervised lifetime is checked first, so keep it clear of the bound
    // under test.
    profile.job_timeout_seconds = 7200;
    let idle = std::time::Duration::from_secs(profile.idle_timeout_seconds);
    let grace = std::time::Duration::from_mins(1);
    let second = std::time::Duration::from_secs(1);

    // A job bound with almost none of the idle TTL left inherits what is left,
    // floored at the post-binding grace — never a second full window.
    let now = tokio::time::Instant::now();
    let window = SupervisionWindow::new(&profile, now + std::time::Duration::from_secs(30));
    assert_eq!(window.overrun(now + grace - second), None);
    assert_eq!(
        window.overrun(now + grace),
        Some("guest runner did not receive a job before its idle timeout")
    );

    // A job bound at once carries the whole budget, and still only that.
    let now = tokio::time::Instant::now();
    let window = SupervisionWindow::new(&profile, now + idle);
    assert_eq!(window.overrun(now + idle - second), None);
    assert_eq!(
        window.overrun(now + idle),
        Some("guest runner did not receive a job before its idle timeout")
    );
}

/// RUN-242: an observation moves the allocation forward or not at all. The
/// runner is normally seen active before it is ever seen idle, so a later idle
/// poll must not ask a running allocation to go back to ready.
#[tokio::test(start_paused = true)]
async fn an_observed_runner_phase_never_moves_backwards() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::config(directory.path().to_path_buf());
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(profile.idle_timeout_seconds);

    let mut window = SupervisionWindow::new(&profile, deadline);
    assert!(window.advance(RunnerStatus::Offline).is_empty());
    assert_eq!(
        window.advance(RunnerStatus::Active),
        [AllocationState::Ready, AllocationState::Running].as_slice()
    );
    assert!(window.is_running());
    assert!(window.advance(RunnerStatus::Idle).is_empty());
    assert!(window.advance(RunnerStatus::Offline).is_empty());
    assert!(window.advance(RunnerStatus::Active).is_empty());
    assert!(window.is_running());

    // Seen idle first, the allocation reports ready once and running once.
    let mut window = SupervisionWindow::new(&profile, deadline);
    assert_eq!(
        window.advance(RunnerStatus::Idle),
        [AllocationState::Ready].as_slice()
    );
    assert!(window.advance(RunnerStatus::Idle).is_empty());
    assert_eq!(
        window.advance(RunnerStatus::Active),
        [AllocationState::Running].as_slice()
    );
    assert!(window.advance(RunnerStatus::Idle).is_empty());
}

/// RUN-242: a runner seen active before it is ever seen idle must survive a
/// later idle poll — a stalled report is not a finished job.
#[tokio::test]
async fn a_late_idle_status_does_not_fail_a_running_job() {
    let fixture = SupervisedFixture::with_statuses(&["offline", "active", "idle"]).await;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "sleep 6; exit 0"])
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let result = fixture.supervise(child).await;

    assert!(result.is_ok(), "supervise: {result:?}");
    assert_eq!(fixture.state().await, AllocationState::Running);
    assert!(fixture.polls() >= 3);
}

/// The same holds for a runner seen idle first: one late idle poll is
/// information, not a reason to fail a healthy job.
#[tokio::test]
async fn an_idle_active_idle_sequence_keeps_the_allocation_running() {
    let fixture = SupervisedFixture::with_statuses(&["idle", "active", "idle"]).await;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "sleep 6; exit 0"])
        .spawn()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let result = fixture.supervise(child).await;

    assert!(result.is_ok(), "supervise: {result:?}");
    assert_eq!(fixture.state().await, AllocationState::Running);
    assert!(fixture.polls() >= 3);
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
                        "run_id": 42,
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

/// SEC-009: a job carrying the published allocation label but belonging to
/// another run must not bind, and must not let repository code kill the
/// allocation either — the phase deadline is the only backstop.
#[tokio::test]
async fn a_job_from_another_run_neither_binds_nor_fails_the_allocation() {
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let application = axum::Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            axum::routing::get(
                |axum::extract::State(requests): axum::extract::State<
                    Arc<std::sync::atomic::AtomicUsize>,
                >| async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(serde_json::json!([{
                        "attempt": 1,
                        "run_id": 43,
                        "handle": "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
                        "name": "apple-build",
                        "runs_on": ["macos-tart-project-allocation"],
                        "status": "waiting"
                    }]))
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
    let worker = FlanForgeWorker::new(&config, forgejo);
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);

    let result = worker
        .wait_for_job_handle(&allocation, &profile, &CancellationToken::new(), deadline)
        .await;

    let message = result.err().map_or_else(
        || unreachable!("a foreign run must not bind"),
        |error| error.to_string(),
    );
    // The deadline is the only backstop. A loaded host can reach it before the
    // unlabelled diagnosis has named the reason, so only the prefix is fixed.
    assert!(
        message.starts_with("authorized Forgejo job did not become ready"),
        "{message}"
    );
    assert!(requests.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}

/// A worker whose Forgejo is `application`, with the allocation and profile the
/// job-wait tests measure against.
async fn job_wait_fixture(
    application: axum::Router,
) -> (FlanForgeWorker, Allocation, Profile, tempfile::TempDir) {
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
    // A loaded host can push a loopback round trip past the fixture's usual 5s,
    // and a timed-out poll is retryable, so the wait would silently start over.
    config.forgejo.http_timeout_seconds = 30;
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
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    (
        FlanForgeWorker::new(&config, forgejo),
        allocation,
        profile,
        directory,
    )
}

/// ARCH-323: Forgejo can never return a dependent job whose `runs-on` carries
/// more than the allocation label, so the wait must say so at once instead of
/// holding a booted VM for the whole idle timeout and blaming Forgejo.
#[tokio::test]
async fn an_unmatchable_dependent_runs_on_fails_the_wait_immediately() {
    let application = axum::Router::new().route(
        "/api/v1/repos/owner/project/actions/runners/jobs",
        axum::routing::get(
            |axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| async move {
                // The server drops a job whose `runs_on` the query does not cover.
                if query.contains_key("labels") {
                    return axum::Json(serde_json::json!(null));
                }
                axum::Json(serde_json::json!([{
                    "attempt": 1,
                    "run_id": 42,
                    "handle": "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
                    "name": "apple-build",
                    "runs_on": ["macos-tart-project-allocation", "macos"],
                    "status": "waiting"
                }]))
            },
        ),
    );
    let (worker, allocation, profile, _directory) = job_wait_fixture(application).await;
    // A deadline the fast failure must never need to reach.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(10);

    // Generous enough for a loaded host, far short of the phase deadline.
    let result = tokio::time::timeout(
        std::time::Duration::from_mins(3),
        worker.wait_for_job_handle(&allocation, &profile, &CancellationToken::new(), deadline),
    )
    .await
    .unwrap_or_else(|_| unreachable!("the wait did not fail fast"));

    assert_eq!(
        result.map_err(|error| error.to_string()),
        Err(
            "dependent Forgejo job \"apple-build\" is queued with runs-on [macos-tart-project-allocation, macos]; the allocation label must be its only entry (expected [\"macos-tart-project-allocation\"])"
                .to_owned()
        )
    );
}

/// ARCH-323: "not queued yet" is a legitimate transient state. The diagnosis
/// must run and must still leave the bounded wait alone, so a slow queue is
/// never mistaken for a misconfigured one.
#[tokio::test]
async fn a_job_that_is_not_queued_yet_keeps_its_bounded_wait() {
    // The notify fires on the first labelled search after a diagnosis, so the
    // wait is observed resuming rather than measured against a wall clock.
    let queries = Arc::new(JobQueries::default());
    let application = axum::Router::new()
        .route(
            "/api/v1/repos/owner/project/actions/runners/jobs",
            axum::routing::get(
                |axum::extract::State(queries): axum::extract::State<Arc<JobQueries>>,
                 axum::extract::Query(query): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| async move {
                    if query.contains_key("labels") {
                        if queries.unlabelled.load(SEQ_CST) >= 1 {
                            queries.resumed.notify_one();
                        }
                    } else {
                        queries.unlabelled.fetch_add(1, SEQ_CST);
                    }
                    axum::Json(serde_json::json!(null))
                },
            ),
        )
        .with_state(queries.clone());
    let (worker, allocation, profile, _directory) = job_wait_fixture(application).await;
    let cancellation = CancellationToken::new();
    // Only a backstop: the wait is cancelled as soon as it is seen polling on.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(3);
    let mut wait =
        std::pin::pin!(worker.wait_for_job_handle(&allocation, &profile, &cancellation, deadline));

    tokio::select! {
        _ = &mut wait => unreachable!("the wait failed instead of waiting for the queue"),
        () = queries.resumed.notified() => (),
    }

    // The unlabelled diagnosis ran, and the wait polled on regardless.
    assert!(queries.unlabelled.load(SEQ_CST) >= 1);
    cancellation.cancel();
    assert_eq!(
        wait.await.map_err(|error| error.to_string()),
        Err("allocation was cancelled".to_owned())
    );
}

const SEQ_CST: std::sync::atomic::Ordering = std::sync::atomic::Ordering::SeqCst;

/// How the fake Forgejo was asked for jobs, and when the wait resumed polling.
#[derive(Default)]
struct JobQueries {
    unlabelled: std::sync::atomic::AtomicUsize,
    resumed: tokio::sync::Notify,
}

/// How the fixture's Forgejo answers a runner status poll.
#[derive(Clone, Copy, Debug)]
enum StatusSource {
    /// An accepted job for the first `n` polls, then a deleted registration.
    ActiveThenAbsent(usize),
    /// These statuses in order, repeating the last one for later polls.
    Sequence(&'static [&'static str]),
}

/// One supervisable allocation whose Forgejo answers status polls from a
/// scripted source.
struct SupervisedFixture {
    worker: FlanForgeWorker,
    allocation: Allocation,
    profile: Profile,
    reporter: AllocationReporter,
    polls: Arc<std::sync::atomic::AtomicUsize>,
    first_absent: Arc<std::sync::atomic::AtomicUsize>,
    manager: AllocationManager,
    _directory: tempfile::TempDir,
}

impl SupervisedFixture {
    async fn new(active_polls: usize) -> Self {
        Self::with_status_source(StatusSource::ActiveThenAbsent(active_polls)).await
    }

    async fn with_statuses(statuses: &'static [&'static str]) -> Self {
        Self::with_status_source(StatusSource::Sequence(statuses)).await
    }

    async fn with_status_source(source: StatusSource) -> Self {
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
                    let active_polls = match source {
                        StatusSource::Sequence(statuses) => {
                            let status = statuses
                                .get(poll - 1)
                                .or_else(|| statuses.last())
                                .copied()
                                .unwrap_or("offline");
                            return Ok(axum::Json(serde_json::json!({"id": 73, "status": status})));
                        }
                        StatusSource::ActiveThenAbsent(active_polls) => active_polls,
                    };
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
            manager,
            _directory: directory,
        }
    }

    /// The state the manager records for the supervised allocation.
    async fn state(&self) -> AllocationState {
        self.manager
            .get_authorized(self.allocation.id, &test_support::claims())
            .await
            .map_or_else(
                |error| unreachable!("fixture: {error}"),
                |allocation| allocation.state,
            )
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

    /// Supervises with the idle budget an untouched job wait would have left.
    async fn supervise(&self, child: tokio::process::Child) -> Result<(), WorkerError> {
        let idle_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(self.profile.idle_timeout_seconds);
        self.worker
            .supervise(
                StartedRunner {
                    process: ssh_process_for_test(child),
                    idle_deadline,
                },
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

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

/// Mints real manager reporters while delegating source evidence to the same
/// Tart adapter the production worker uses.
#[derive(Debug)]
struct TartReportingWorker {
    sender: tokio::sync::mpsc::UnboundedSender<AllocationReporter>,
    tart: TartClient,
}

#[async_trait::async_trait]
impl AllocationWorker for TartReportingWorker {
    fn is_clone_source_evidence_deferred(&self) -> bool {
        true
    }

    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        let _ = self.sender.send(reporter);
        cancellation.cancelled().await;
        Err(WorkerError::new("cancelled"))
    }

    async fn machines(&self) -> Result<Vec<flanforge_manager::HostMachine>, WorkerError> {
        self.tart.host_machines().await
    }

    async fn base_fingerprint(&self, template: &VmName) -> Option<BaseFingerprint> {
        self.tart.fingerprint(template).await
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

async fn waiting_allocation(
    config: &Arc<Config>,
    directory: &tempfile::TempDir,
) -> (AllocationManager, Allocation, AllocationReporter) {
    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let images: Arc<dyn WarmImageStore> = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(config)),
        store,
        images,
        hot,
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

/// `storage_mb` is the total virtual disk of the ephemeral guest, so a
/// profile larger than its base grows the clone before it boots.
#[test]
fn a_profile_larger_than_its_base_grows_the_clone_disk() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    // 16 GiB of base under a 40 GiB profile: 42_949_672_960 bytes, which is 43
    // of the decimal gigabytes Tart counts in.
    let (log, logged) = sized_clone(&directory, 16 * GIB, 40 * 1_024);
    assert_eq!(
        log,
        "set ci-project-42-1 --cpu 4 --memory 8192 --disk-size 43\n"
    );
    assert!(!logged.contains("smaller than the cloned base"), "{logged}");
}

/// Tart disks only grow, so a profile below its base is honoured at the base
/// size and said so rather than refused.
#[test]
fn a_profile_smaller_than_its_base_keeps_the_base_disk() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (log, logged) = sized_clone(&directory, 20 * GIB, 16 * 1_024);
    assert_eq!(log, "set ci-project-42-1 --cpu 4 --memory 8192\n");
    assert!(logged.contains("smaller than the cloned base"), "{logged}");
    for value in ["requested_mb=16384", "base_mb=20480", "effective_mb=20480"] {
        assert!(logged.contains(value), "{value}: {logged}");
    }
}

/// A library that cannot be located leaves the disk exactly as it is, rather
/// than guessing a size onto it.
#[test]
fn an_unlocatable_library_leaves_the_clone_disk_alone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(&directory, "[]");
    let (allocation, profile) = sized_allocation(40 * 1_024);
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (result, logged) = test_support::capture_logs(|| {
        runtime.block_on(
            TartClient::with_library(config.runtime, None).configure(&allocation, &profile),
        )
    });
    result.unwrap_or_else(|error| unreachable!("configure: {error}"));
    assert_eq!(
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("log: {error}")),
        "set ci-project-42-1 --cpu 4 --memory 8192\n"
    );
    assert!(logged.contains("no Tart disk can be measured"), "{logged}");
}

/// The unit Tart counts in: whole decimal gigabytes, never a shrink.
#[test]
fn a_disk_size_argument_rounds_up_and_never_shrinks() {
    for (requested, base, expected, reason) in [
        (
            42_949_672_960,
            16 * GIB,
            Some(43),
            "40 GiB over a 16 GiB base",
        ),
        (
            1_000_000_001,
            1_000_000_000,
            Some(2),
            "one byte over a whole GB",
        ),
        (16 * GIB, 20 * GIB, None, "a request below its base"),
        (20 * GIB, 20 * GIB, None, "a request equal to its base"),
        (u64::MAX, 20 * GIB, None, "a request Tart cannot express"),
    ] {
        assert_eq!(disk_size_argument(requested, base), expected, "{reason}");
    }
}

/// Runs `configure` against a clone whose disk is `base_bytes`, and reports the
/// Tart invocations and what the daemon logged.
fn sized_clone(
    directory: &tempfile::TempDir,
    base_bytes: u64,
    storage_mb: u64,
) -> (String, String) {
    let (script, log) = fake_tart(directory, "[]");
    let library = directory.path().join("library");
    let clone = library.join("vms").join("ci-project-42-1");
    std::fs::create_dir_all(&clone).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::File::create(clone.join("disk.img"))
        .and_then(|disk| disk.set_len(base_bytes))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (allocation, profile) = sized_allocation(storage_mb);
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (result, logged) = test_support::capture_logs(|| {
        runtime.block_on(
            TartClient::with_library(config.runtime, Some(library))
                .configure(&allocation, &profile),
        )
    });
    result.unwrap_or_else(|error| unreachable!("configure: {error}"));
    (
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("log: {error}")),
        logged,
    )
}

fn sized_allocation(storage_mb: u64) -> (Allocation, Profile) {
    let allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-42-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("macos-tart-project-allocation")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        flanforge_core::GuestSize {
            storage_mb,
            ..test_support::size()
        },
    );
    (allocation, test_support::profile())
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
    let log = directory.path().join("mutations.log");
    let source = format!(
        "if [ \"$1\" = list ]; then\n  /usr/bin/printf '%s' {}\nelse\n  /usr/bin/printf '%s\\n' \"$*\" >> {}\n  [ \"$1\" != {} ] || exit 1\nfi\n",
        shell_quote(machines),
        shell_quote(&log.to_string_lossy()),
        shell_quote(failing),
    );
    let script = test_support::executable(directory.path(), "tart", &source);
    (script, log)
}

struct StatefulTart {
    script: std::path::PathBuf,
    state: std::path::PathBuf,
    clone_started: std::path::PathBuf,
    clone_release: std::path::PathBuf,
    consumer_generation: std::path::PathBuf,
    mutations: std::path::PathBuf,
}

fn stateful_race_tart(directory: &tempfile::TempDir) -> StatefulTart {
    let state = directory.path().join("tart-state");
    let clone_started = directory.path().join("consumer-clone-started");
    let clone_release = directory.path().join("consumer-clone-release");
    let consumer_generation = directory.path().join("consumer-generation");
    let mutations = directory.path().join("race-mutations.log");
    std::fs::write(
        &state,
        "flanforge-base|stopped|base\nproject-warm|stopped|generation-a\n",
    )
    .unwrap_or_else(|error| unreachable!("fixture state: {error}"));
    let source = format!(
        r#"state={state}
started={started}
release={release}
captured={captured}
mutations={mutations}
list_state() {{
  first=1
  /usr/bin/printf '['
  while IFS='|' read -r name status generation; do
    [ -n "$name" ] || continue
    [ "$first" -eq 1 ] || /usr/bin/printf ','
    first=0
    /usr/bin/printf '{{"Name":"%s","State":"%s"}}' "$name" "$status"
  done < "$state"
  /usr/bin/printf ']'
}}
replace() {{
  name=$1 status=$2 generation=$3
  /usr/bin/awk -F '|' -v name="$name" '$1 != name' "$state" > "$state.next"
  /usr/bin/printf '%s|%s|%s\n' "$name" "$status" "$generation" >> "$state.next"
  /bin/mv "$state.next" "$state"
}}
remove() {{
  name=$1
  /usr/bin/awk -F '|' -v name="$name" '$1 != name' "$state" > "$state.next"
  /bin/mv "$state.next" "$state"
}}
generation() {{
  /usr/bin/awk -F '|' -v name="$1" '$1 == name {{ print $3; exit }}' "$state"
}}
case "$1" in
  list) list_state ;;
  clone)
    /usr/bin/printf '%s\n' "$*" >> "$mutations"
    if [ "$2" = project-warm ] && case "$3" in ci-*) true;; *) false;; esac; then
      : > "$started"
      while [ ! -f "$release" ]; do /bin/sleep 0.01; done
    fi
    value="$(generation "$2")"
    [ -n "$value" ] || exit 1
    replace "$3" stopped "$value"
    if [ "$2" = project-warm ] && case "$3" in ci-*) true;; *) false;; esac; then
      /usr/bin/printf '%s\n' "$value" > "$captured"
    fi
    ;;
  delete)
    /usr/bin/printf '%s\n' "$*" >> "$mutations"
    [ "$2" != project-warm.staging ] || exit 1
    remove "$2"
    ;;
  stop)
    /usr/bin/printf '%s\n' "$*" >> "$mutations"
    value="$(generation "$2")"
    replace "$2" stopped "$value"
    ;;
  *) /usr/bin/printf '%s\n' "$*" >> "$mutations" ;;
esac
"#,
        state = shell_quote(&state.to_string_lossy()),
        started = shell_quote(&clone_started.to_string_lossy()),
        release = shell_quote(&clone_release.to_string_lossy()),
        captured = shell_quote(&consumer_generation.to_string_lossy()),
        mutations = shell_quote(&mutations.to_string_lossy()),
    );
    StatefulTart {
        script: test_support::executable(directory.path(), "stateful-tart", &source),
        state,
        clone_started,
        clone_release,
        consumer_generation,
        mutations,
    }
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| unreachable!("fixture did not create {}", path.display()));
}

/// A Tart whose first listing is full and whose later ones are not, so a test
/// can observe the capacity wait without racing a real VM. Returns the script
/// and the file that counts `list` calls.
pub(crate) fn fake_tart_freeing(
    directory: &tempfile::TempDir,
    full: &str,
    free: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let calls = directory.path().join("list.calls");
    let source = format!(
        "if [ \"$1\" = list ]; then\n  /usr/bin/printf 'x\\n' >> {calls}\n  if [ \"$(/usr/bin/wc -l < {calls})\" -le 1 ]; then\n    /usr/bin/printf '%s' {full}\n  else\n    /usr/bin/printf '%s' {free}\n  fi\nfi\n",
        calls = shell_quote(&calls.to_string_lossy()),
        full = shell_quote(full),
        free = shell_quote(free),
    );
    let script = test_support::executable(directory.path(), "tart", &source);
    (script, calls)
}

/// The gate is fixed: no workflow value can reach the guest shell.
#[test]
fn the_retention_marker_check_interpolates_no_workflow_data() {
    let script = retention_marker_script();
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
    let expanded = script.replace("$HOME", "").replace("$marker", "");
    assert!(!expanded.contains('$'), "{expanded}");
}

#[test]
fn the_retention_marker_check_is_narrow_and_read_only() {
    let script = retention_marker_script();
    assert!(script.contains(REGENERATION_SENTINEL));
    for mutation in ["rm ", "mv ", "cp ", "truncate", "logout", "*", "?"] {
        assert!(!script.contains(mutation), "{mutation}: {script}");
    }
    for unrelated in [".git", ".ssh", "_work", "tailscale", "/tmp"] {
        assert!(!script.contains(unrelated), "{unrelated}: {script}");
    }
}

fn run_retention_marker_check(home: &std::path::Path) -> Option<i32> {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(retention_marker_script())
        .env("HOME", home)
        .status()
        .unwrap_or_else(|error| unreachable!("marker check: {error}"))
        .code()
}

#[test]
fn a_missing_or_non_regular_marker_rejects_retention() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let home = directory.path();
    let marker = home.join(REGENERATION_SENTINEL);

    assert_ne!(run_retention_marker_check(home), Some(0));
    std::fs::create_dir(&marker).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_ne!(run_retention_marker_check(home), Some(0));
    std::fs::remove_dir(&marker).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::write(home.join("marker-target"), b"complete")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::os::unix::fs::symlink("marker-target", &marker)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_ne!(run_retention_marker_check(home), Some(0));
}

#[test]
fn a_regular_marker_passes_without_mutating_the_guest_home() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let home = directory.path();
    let marker = home.join(REGENERATION_SENTINEL);
    let credential = home.join(".git-credentials");
    std::fs::write(&marker, b"complete").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    std::fs::write(&credential, b"preserved")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert_eq!(run_retention_marker_check(home), Some(0));
    assert_eq!(
        std::fs::read(marker).ok().as_deref(),
        Some(b"complete".as_slice())
    );
    assert_eq!(
        std::fs::read(credential).ok().as_deref(),
        Some(b"preserved".as_slice())
    );
}

#[tokio::test]
async fn a_failed_marker_check_aborts_retention() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    config.runtime.ssh_path = "/bin/false".into();
    let worker = FlanForgeWorker::new(
        &config,
        ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );

    assert!(worker.ensure_retention_marker("192.0.2.10").await.is_err());
    assert!(worker.ensure_retention_marker("not-an-ip").await.is_err());
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
    tart_config(&mut config).path = script;
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
        .ensure_can_clone(&allocation, &profile, clone_deadline())
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
    tart_config(&mut config).path = script;
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
        .ensure_can_clone(&allocation, &test_support::warm_profile(), clone_deadline())
        .await
        .unwrap_or_else(|error| unreachable!("prepare clone: {error}"));
    assert_eq!(source.name, test_support::warm_profile().template);
}

#[tokio::test]
async fn a_rebuilt_base_changes_its_fingerprint_and_an_unreadable_one_has_none() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).home = Some(directory.path().to_path_buf());
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
#[expect(
    clippy::too_many_lines,
    reason = "the production race sequence and its persisted evidence stay visible together"
)]
async fn failed_promotion_stays_serial_with_clone_and_persists_the_live_generation() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let fake = stateful_race_tart(&directory);
    let library = directory.path().join("tart-library");
    let base = library.join("vms/flanforge-base");
    std::fs::create_dir_all(&base).unwrap_or_else(|error| unreachable!("fixture library: {error}"));
    std::fs::write(base.join("disk.img"), b"generation-a")
        .unwrap_or_else(|error| unreachable!("fixture disk: {error}"));
    std::fs::write(base.join("config.json"), b"{}")
        .unwrap_or_else(|error| unreachable!("fixture config: {error}"));

    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    let profile = config
        .profiles
        .get_mut(&test_support::profile_name())
        .unwrap_or_else(|| unreachable!("fixture profile"));
    profile.allowed_workflows.insert("consumer.yml".to_owned());
    let profile = profile.clone();
    let tart = tart_config(&mut config);
    tart.path = fake.script.clone();
    tart.home = Some(library);
    config.runtime.ssh_path = "/bin/true".into();
    config.runtime.max_running_vms = 4;
    config.runtime.host_cpu_count = Some(16);
    config.runtime.host_memory_mb = Some(32_768);
    let config = Arc::new(config);
    let forgejo = ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
        .unwrap_or_else(|error| unreachable!("fixture client: {error}"));
    let worker = FlanForgeWorker::new(&config, forgejo);
    let initial_fingerprint = worker
        .tart
        .fingerprint(&profile.template)
        .await
        .unwrap_or_else(|| unreachable!("fixture base fingerprint"));
    let candidate_fingerprint = BaseFingerprint::new("bb02")
        .unwrap_or_else(|error| unreachable!("fixture fingerprint: {error}"));

    let store: Arc<dyn AllocationStore> = Arc::new(
        SqliteAllocationStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture store: {error}")),
    );
    let images = Arc::new(
        SqliteWarmImageStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture images: {error}")),
    );
    let hot = Arc::new(
        SqliteHotGuestStore::open(&directory.path().join("state.db"))
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let producer_id = flanforge_core::AllocationId::new();
    images
        .save(&WarmImageRecord {
            profile: test_support::profile_name(),
            warm_template: profile
                .warm_template
                .clone()
                .unwrap_or_else(|| unreachable!("fixture warm image")),
            generation: 1,
            base_fingerprint: initial_fingerprint.clone(),
            produced_by: producer_id,
            produced_at_unix: 1_755_000_000,
            state: WarmImageState::Promoted,
            previous: None,
        })
        .await
        .unwrap_or_else(|error| unreachable!("fixture warm record: {error}"));
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        store,
        images.clone(),
        hot,
        Arc::new(TartReportingWorker {
            sender,
            tart: TartClient::new(config.runtime.clone()),
        }),
    );

    let mut consumer_claims = test_support::claims();
    consumer_claims.workflow = "consumer.yml".into();
    consumer_claims.workflow_ref =
        "owner/project/.forgejo/workflows/consumer.yml@refs/heads/main".into();
    let consumer = manager
        .create(
            test_support::request(),
            RequestOptions {
                warm: true,
                hot: flanforge_core::HotRequest::Untouched,
                ..RequestOptions::default()
            },
            &consumer_claims,
        )
        .await
        .unwrap_or_else(|error| unreachable!("consumer create: {error}"))
        .allocation;
    let consumer_reporter = receiver
        .recv()
        .await
        .unwrap_or_else(|| unreachable!("consumer reporter"));
    let mut producer_request = test_support::request();
    producer_request.run_id = 43;
    let mut producer_claims = test_support::claims();
    producer_claims.run_id = "43".into();
    let mut producer = manager
        .create(
            producer_request,
            RequestOptions::default(),
            &producer_claims,
        )
        .await
        .unwrap_or_else(|error| unreachable!("producer create: {error:?}"))
        .allocation;
    let producer_reporter = receiver
        .recv()
        .await
        .unwrap_or_else(|| unreachable!("producer reporter"));
    producer.set_clone_source(
        CloneSource {
            name: profile.template.clone(),
            kind: CloneKind::Template,
            base_fingerprint: Some(candidate_fingerprint.clone()),
            fallback_reason: None,
        },
        None,
    );
    let mut state = std::fs::OpenOptions::new()
        .append(true)
        .open(&fake.state)
        .unwrap_or_else(|error| unreachable!("fixture state: {error}"));
    writeln!(state, "{}|running|generation-b", producer.vm_name)
        .unwrap_or_else(|error| unreachable!("fixture state: {error}"));

    let clone_worker = worker.clone();
    let clone_profile = profile.clone();
    let clone_reporter = consumer_reporter.clone();
    let clone_cancellation = CancellationToken::new();
    let consumer_task = tokio::spawn(async move {
        let mut consumer = consumer;
        clone_worker
            .clone_current_source(
                &mut consumer,
                &clone_profile,
                &clone_reporter,
                &clone_cancellation,
                clone_deadline(),
            )
            .await
            .unwrap_or_else(|error| unreachable!("consumer clone: {error}"));
        consumer
    });
    worker.wait_for_image_lock_attempts(1).await;
    wait_for_file(&fake.clone_started).await;

    let retention_worker = worker.clone();
    let retention_profile = profile.clone();
    let retention_reporter = producer_reporter.clone();
    let retention_cancellation = CancellationToken::new();
    let previous = WarmGeneration {
        generation: 1,
        base_fingerprint: initial_fingerprint.clone(),
        produced_by: producer_id,
        produced_at_unix: 1_755_000_000,
    };
    let promotion = tokio::spawn(async move {
        let mut child = tokio::process::Command::new("/bin/true")
            .spawn()
            .unwrap_or_else(|error| unreachable!("fixture child: {error}"));
        let request = RetentionRequest {
            allocation: &producer,
            profile: &retention_profile,
            warm_template: retention_profile
                .warm_template
                .clone()
                .unwrap_or_else(|| unreachable!("fixture warm image")),
            base_fingerprint: candidate_fingerprint,
            generation: 2,
            previous: Some(previous),
        };
        let outcome = retention_worker
            .retain_warm(
                request,
                &mut child,
                "192.0.2.10",
                &retention_reporter,
                &retention_cancellation,
            )
            .await;
        retention_reporter
            .set_retention(outcome.clone())
            .await
            .unwrap_or_else(|error| unreachable!("retention report: {error}"));
        (producer, outcome)
    });
    worker.wait_for_image_lock_attempts(2).await;
    std::fs::write(&fake.clone_release, b"release")
        .unwrap_or_else(|error| unreachable!("fixture release: {error}"));

    let consumer = consumer_task
        .await
        .unwrap_or_else(|error| unreachable!("consumer task: {error}"));
    let (producer, outcome) = promotion
        .await
        .unwrap_or_else(|error| unreachable!("promotion task: {error}"));
    assert_eq!(
        std::fs::read_to_string(&fake.consumer_generation)
            .unwrap_or_else(|error| unreachable!("captured generation: {error}"))
            .trim(),
        "generation-a"
    );
    assert_eq!(consumer.warm_generation, Some(1));
    assert_eq!(
        consumer
            .source
            .as_ref()
            .and_then(|source| source.base_fingerprint.as_ref()),
        Some(&initial_fingerprint)
    );
    assert_eq!(outcome.outcome, flanforge_core::RetentionResult::Promoted);
    assert_eq!(outcome.generation, Some(2));

    let record = images
        .load(&test_support::profile_name())
        .await
        .unwrap_or_else(|error| unreachable!("load warm record: {error}"))
        .unwrap_or_else(|| unreachable!("warm record disappeared"));
    assert_eq!(record.generation, 2);
    assert_eq!(
        record.base_fingerprint,
        candidate_fingerprint_for_assertion()
    );
    assert_eq!(record.state, WarmImageState::Promoted);
    let reported = manager
        .get_authorized(producer.id, &producer_claims)
        .await
        .unwrap_or_else(|error| unreachable!("producer evidence: {error}"));
    assert_eq!(
        reported
            .retention
            .as_ref()
            .and_then(|value| value.generation),
        Some(2)
    );
    let live = std::fs::read_to_string(&fake.state)
        .unwrap_or_else(|error| unreachable!("live state: {error}"));
    assert!(
        live.lines()
            .any(|line| line == "project-warm|stopped|generation-b")
    );
    let mutations = std::fs::read_to_string(&fake.mutations)
        .unwrap_or_else(|error| unreachable!("mutations: {error}"));
    let consumer_clone = mutations
        .lines()
        .position(|line| line.starts_with("clone project-warm ci-"))
        .unwrap_or_else(|| unreachable!("consumer clone mutation"));
    let promotion_stage = mutations
        .lines()
        .position(|line| line.ends_with(" project-warm.staging"))
        .unwrap_or_else(|| unreachable!("promotion stage mutation"));
    assert!(
        consumer_clone < promotion_stage,
        "promotion crossed clone boundary"
    );
}

fn candidate_fingerprint_for_assertion() -> BaseFingerprint {
    BaseFingerprint::new("bb02")
        .unwrap_or_else(|error| unreachable!("fixture fingerprint: {error}"))
}

/// RUN-727 charges only `Owned` name matches, so an unattested clone would be
/// counted foreign against its own daemon and strand every later allocation.
#[tokio::test]
async fn machine_ownership_follows_the_configured_prefix() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-42-1","State":"running"},{"Name":"someone-elses","State":"running"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
    let machines = TartClient::new(config.runtime)
        .host_machines()
        .await
        .unwrap_or_else(|error| unreachable!("machines: {error}"));

    let ownership = |name: &str| {
        machines
            .iter()
            .find(|machine| machine.name == name)
            .map(|machine| machine.ownership)
    };
    assert_eq!(ownership("ci-project-42-1"), Some(MachineOwnership::Owned));
    assert_eq!(ownership("someone-elses"), Some(MachineOwnership::Foreign));
}

/// RUN-102: the library is resolved the way Tart resolves it, so an unset
/// `runtime.backend.home` leaves a candidate ageable rather than orphaned.
/// `TART_HOME` is only honoured when it is usable: the `tart` child would obey
/// an unusable one too, so falling through to `~/.tart` would name a library
/// Tart is not reading.
#[test]
fn the_tart_library_is_derived_when_no_home_is_configured() {
    let configured = std::path::Path::new("/srv/configured");
    let inherited = std::path::Path::new("/srv/inherited");
    let home = std::path::Path::new("/home/service");

    assert_eq!(
        library_home(Some(configured), Some(inherited), Some(home)),
        Some(configured.to_path_buf()),
        "configuration outranks the environment"
    );
    assert_eq!(
        library_home(None, Some(inherited), Some(home)),
        Some(inherited.to_path_buf()),
        "TART_HOME is what the child will read"
    );
    assert_eq!(
        library_home(None, None, Some(home)),
        Some(home.join(".tart")),
        "Tart's own default library"
    );
    assert_eq!(
        library_home(
            None,
            Some(std::path::Path::new("relative/tart")),
            Some(home)
        ),
        None,
        "an unusable TART_HOME resolves to nothing, never to ~/.tart"
    );
    assert_eq!(
        library_home(None, None, None),
        None,
        "nothing left to derive from"
    );
}

/// RUN-102: the client wires that resolution to its own configuration and to
/// the process environment. The order it resolves in is covered separately, by
/// `the_tart_library_is_derived_when_no_home_is_configured`.
#[test]
fn the_client_resolves_its_library_from_the_process_environment() {
    let mut config = (*test_support::config("/tmp/flanforge-library".into())).clone();
    tart_config(&mut config).home = None;
    let environment = library_home(
        None,
        std::env::var_os("TART_HOME")
            .map(std::path::PathBuf::from)
            .as_deref(),
        flanforge_paths::service_home().ok().as_deref(),
    );
    assert_eq!(
        TartClient::new(config.runtime.clone())
            .library()
            .map(std::path::Path::to_path_buf),
        environment,
        "an unconfigured client takes the environment's library"
    );

    tart_config(&mut config).home = Some("/srv/configured".into());
    assert_eq!(
        TartClient::new(config.runtime).library(),
        Some(std::path::Path::new("/srv/configured")),
        "configuration still outranks the environment"
    );
}

/// RUN-102: an orphan on a host with no configured `runtime.backend.home` is
/// aged from the derived library, so the sweep can reach it.
#[tokio::test]
async fn a_vm_is_aged_from_the_derived_library_without_a_configured_home() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"stopped"}]"#,
    );
    let library = directory.path().join("derived-library");
    std::fs::create_dir_all(library.join("vms").join("ci-project-41-1"))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    let tart = tart_config(&mut config);
    tart.path = script;
    tart.home = None;

    let machines = TartClient::with_library(config.runtime, Some(library))
        .host_machines()
        .await
        .unwrap_or_else(|error| unreachable!("machines: {error}"));

    assert_eq!(machines.len(), 1);
    assert!(
        machines
            .first()
            .and_then(|machine| machine.age_seconds)
            .is_some(),
        "a derived library still ages the clone: {machines:?}"
    );
}

/// RUN-102: the whole chain on a host that never configured a library — the
/// derived location gives the clone an age, the age clears the minimum-age
/// gate, and the planner authorizes the orphan by prefix.
#[tokio::test]
async fn an_orphan_is_collectable_through_the_derived_library() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"stopped"}]"#,
    );
    let library = directory.path().join("derived-library");
    let clone = library.join("vms").join("ci-project-41-1");
    std::fs::create_dir_all(&clone).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    backdate(&clone);
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    let tart = tart_config(&mut config);
    tart.path = script;
    tart.home = None;
    let runtime = config.runtime.clone();

    let machines = TartClient::with_library(runtime, Some(library))
        .host_machines()
        .await
        .unwrap_or_else(|error| unreachable!("machines: {error}"));
    let planned = flanforge_manager::plan_sweep(flanforge_manager::ReapInputs {
        config: &config,
        machines: &machines,
        allocations: &[],
        images: &[],
        hot: &[],
    });

    assert_eq!(planned.len(), 1, "{planned:?}");
    let candidate = planned
        .first()
        .unwrap_or_else(|| unreachable!("one candidate"));
    assert_eq!(candidate.name, "ci-project-41-1");
    assert_eq!(
        candidate.authorization,
        flanforge_manager::ReapAuthorization::Prefix
    );
}

/// Ages a fixture directory past the sweep's minimum-age gate. `touch -t` is
/// the portable way to set one; `std::fs` cannot set a directory's mtime.
fn backdate(path: &std::path::Path) {
    let status = std::process::Command::new("touch")
        .args(["-t", "202001010000"])
        .arg(path)
        .status()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(status.success(), "could not age the fixture directory");
}

/// RUN-102: a library that cannot be located, and one that cannot account for a
/// listed VM, both leave the age unknown. Inventing one would defeat the
/// minimum-age gate, so the sweep reports these names instead of deleting them.
#[tokio::test]
async fn an_undeterminable_age_is_reported_as_unknown() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, _log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"stopped"}]"#,
    );
    let mut config = (*test_support::config(directory.path().to_path_buf())).clone();
    let tart = tart_config(&mut config);
    tart.path = script;
    tart.home = None;

    for library in [None, Some(directory.path().join("empty-library"))] {
        let machines = TartClient::with_library(config.runtime.clone(), library.clone())
            .host_machines()
            .await
            .unwrap_or_else(|error| unreachable!("machines: {error}"));
        assert_eq!(machines.len(), 1);
        assert_eq!(
            machines.first().and_then(|machine| machine.age_seconds),
            None,
            "{library:?} must not produce an age"
        );
    }
}

#[tokio::test]
async fn the_runtime_refuses_a_deletion_no_record_authorizes() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"stopped"},{"Name":"flanforge-base","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
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
    let foreign =
        VmName::new("someones-laptop").unwrap_or_else(|error| unreachable!("fixture: {error}"));

    // RUN-102: prefix authority reaches inside the boundary and nowhere else.
    assert!(
        worker
            .delete_vm(flanforge_manager::ReapRequest {
                name: &foreign,
                authorization: &flanforge_manager::ReapAuthorization::Prefix,
                profile: None,
                record: None,
                allocation: None,
                reserved: &reserved,
                budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
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
                allocation: None,
                reserved: &reserved,
                budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
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
            allocation: None,
            reserved: &reserved,
            budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
        })
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete ci-project-41-1\n");
}

/// RUN-102: an orphan whose record has aged out is still the daemon's, because
/// the configured prefix says so, and a name live configuration claims is not.
#[tokio::test]
async fn the_prefix_alone_authorizes_an_orphan_and_never_a_declared_name() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let (script, log) = fake_tart(
        &directory,
        r#"[{"Name":"ci-project-41-1","State":"running"},{"Name":"project-warm","State":"stopped"}]"#,
    );
    let mut config = (*test_support::warm_config(directory.path().to_path_buf())).clone();
    tart_config(&mut config).path = script;
    let worker = FlanForgeWorker::new(
        &config,
        ForgejoClient::new(Arc::new(config.forgejo.clone()), "token".into())
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    let reserved = flanforge_manager::reserved_image_names(&config);
    let orphan =
        VmName::new("ci-project-41-1").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let declared =
        VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}"));

    assert!(
        worker
            .delete_vm(flanforge_manager::ReapRequest {
                name: &declared,
                authorization: &flanforge_manager::ReapAuthorization::Prefix,
                profile: None,
                record: None,
                allocation: None,
                reserved: &reserved,
                budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
            })
            .await
            .is_err()
    );
    assert!(!log.exists(), "an unauthorized deletion reached Tart");

    worker
        .delete_vm(flanforge_manager::ReapRequest {
            name: &orphan,
            authorization: &flanforge_manager::ReapAuthorization::Prefix,
            profile: None,
            record: None,
            allocation: None,
            reserved: &reserved,
            budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
        })
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
    let mutations =
        std::fs::read_to_string(log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "stop ci-project-41-1\ndelete ci-project-41-1\n");
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
    tart_config(&mut config).path = script;
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
            allocation: None,
            reserved: &reserved,
            budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
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
                allocation: None,
                reserved: &reserved,
                budget: CleanupBudget::allow(std::time::Duration::from_secs(10)),
            })
            .await
            .is_err()
    );
    let mutations =
        std::fs::read_to_string(&log).unwrap_or_else(|error| unreachable!("mutation log: {error}"));
    assert_eq!(mutations, "delete project-retired\n");
}
