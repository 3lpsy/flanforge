use std::time::Duration;

use flanforge_libvirt_wire::HelperRequest;
use uuid::Uuid;

use super::{
    exec::{JobAccount, login_shell, program, stop_unit, transient_unit, unit_name},
    ready::{GuestReadiness, ReadyGate},
};

fn account() -> JobAccount {
    JobAccount::new("runner".to_owned(), 2_000)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

/// The agent executes as root. Without the wrapper every job would run as root
/// too, which is a privilege escalation introduced by a channel choice.
#[test]
fn every_script_runs_as_the_job_account_with_a_usable_session() {
    let request = login_shell(&account(), "id -un", None, false);
    assert_eq!(request.path, "/usr/sbin/runuser");
    assert_eq!(request.arguments[0], "-l");
    assert_eq!(request.arguments[1], "runner");
    assert_eq!(request.arguments[2], "-c");
    let script = &request.arguments[3];
    assert!(script.contains("export XDG_RUNTIME_DIR=/run/user/2000;"));
    assert!(script.contains("export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/2000/bus;"));
    assert!(script.ends_with("id -un"));
    assert!(!script.contains('\0'));
}

/// A privileged or malformed account never becomes an argv: the base image's
/// recorded uid is the authority, and it is checked before anything is built.
#[test]
fn a_privileged_or_malformed_job_account_is_refused() {
    for (name, uid) in [
        ("root", 0_u32),
        ("root", 2_000),
        ("runner", 999),
        ("-runner", 2_000),
        ("Runner", 2_000),
        ("runner;id", 2_000),
        ("", 2_000),
    ] {
        assert!(
            JobAccount::new(name.to_owned(), uid).is_err(),
            "accepted {name}:{uid}"
        );
    }
    assert!(JobAccount::new("runner".to_owned(), 1_000).is_ok());
}

/// The job is a transient unit PID 1 owns, so its name, its lifetime, and its
/// stop are all outside the job account's reach.
#[test]
fn the_job_is_a_transient_unit_with_a_guest_side_deadline() {
    let handle = Uuid::new_v4();
    let request = transient_unit(
        &account(),
        "run-the-job",
        handle,
        Duration::from_mins(30),
        Some(b"token\n".to_vec()),
    );
    assert_eq!(request.path, "/usr/bin/systemd-run");
    assert_eq!(
        request.arguments[0],
        format!("--unit={}", unit_name(handle))
    );
    assert_eq!(request.arguments[1], "--pipe");
    assert_eq!(request.arguments[2], "--wait");
    assert_eq!(request.arguments[3], "--property=RuntimeMaxSec=1800");
    assert_eq!(request.arguments[4], "--property=TimeoutStopSec=10");
    assert_eq!(request.arguments[5], "/usr/sbin/runuser");
    assert!(!request.is_output_captured);
    assert_eq!(request.input.as_deref(), Some(b"token\n".as_slice()));
}

/// The wire refuses a zero or unbounded guest-side deadline, so the channel
/// never asks for one it could not send.
#[test]
fn the_guest_side_deadline_is_clamped_to_what_the_wire_accepts() {
    let handle = Uuid::new_v4();
    for (lifetime, expected) in [
        (Duration::ZERO, "--property=RuntimeMaxSec=1"),
        (Duration::from_secs(1), "--property=RuntimeMaxSec=1"),
        (
            Duration::from_secs(999_999),
            "--property=RuntimeMaxSec=86400",
        ),
    ] {
        let request = transient_unit(&account(), "job", handle, lifetime, None);
        assert_eq!(request.arguments[3], expected);
    }
}

/// Every request this channel can build has to survive the wire's argv-shape
/// validation; a shape only one side accepts would fail at the first
/// allocation instead of here.
#[test]
fn every_built_request_passes_the_helper_contract() {
    let handle = Uuid::new_v4();
    let arguments = ["--wait-seconds".to_owned(), "20".to_owned()];
    let requests = [
        login_shell(&account(), "tailscale-join", Some(b"key\n".to_vec()), false),
        login_shell(&account(), "check-the-runner", None, false),
        transient_unit(
            &account(),
            "one-job",
            handle,
            Duration::from_mins(30),
            Some(b"token\n".to_vec()),
        ),
        stop_unit(handle),
        program(
            "/usr/local/libexec/flanforge-guest-ready",
            &arguments,
            None,
            true,
        ),
    ];
    for request in requests {
        let path = request.path.clone();
        let encoded = HelperRequest::AgentExec {
            config: helper_config(),
            manifest: ownership_manifest(),
            request,
            timeout_seconds: 5,
        }
        .encode();
        assert!(encoded.is_ok(), "the helper refused {path}");
    }
}

#[test]
fn a_stop_names_only_this_allocations_own_unit() {
    let handle = Uuid::new_v4();
    let request = stop_unit(handle);
    assert_eq!(request.path, "/usr/bin/systemctl");
    assert_eq!(request.arguments, vec!["stop", &unit_name(handle)]);
    assert!(request.input.is_none());
}

#[test]
fn a_readiness_report_is_parsed_within_its_own_bounds() {
    let ready = br#"{"schema":1,"contract":2,"ready":true,"gate":"ready","state":"ok","detail":"","failed_units":[],"job_account":{"name":"runner","uid":2000}}"#;
    let report =
        GuestReadiness::parse(ready, 2).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(report.is_ready());
    assert_eq!(report.gate(), ReadyGate::Ready);
    assert_eq!(report.account(), ("runner", 2_000));
}

/// A named cause in seconds is the whole point of this probe: a boot timeout
/// reports nothing at all.
#[test]
fn a_failed_gate_reports_the_cause_the_guest_named() {
    let failed = br#"{"schema":1,"contract":2,"ready":false,"gate":"cloud-init","state":"error","detail":"Invalid format at line 7 column 3","failed_units":[],"job_account":{"name":"runner","uid":2000}}"#;
    let report =
        GuestReadiness::parse(failed, 2).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(!report.is_ready());
    assert_eq!(
        report.cause(),
        "guest provisioning failed at gate cloud-init (error): Invalid format at line 7 column 3"
    );

    let units = br#"{"schema":1,"contract":2,"ready":false,"gate":"units","state":"inactive","detail":"required guest units are not active","failed_units":["tailscaled.service","qemu-guest-agent.service"],"job_account":{"name":"runner","uid":2000}}"#;
    let report =
        GuestReadiness::parse(units, 2).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(
        report
            .cause()
            .ends_with("[tailscaled.service, qemu-guest-agent.service]")
    );
}

/// The helper bounds its own output; re-bounding here is what makes the
/// guest's bytes data rather than trust.
#[test]
fn a_report_outside_the_contract_is_refused_rather_than_repeated() {
    let refused: [&[u8]; 8] = [
        b"",
        b"not json",
        // A gate outside the closed set, and an unknown field.
        br#"{"schema":1,"contract":2,"ready":true,"gate":"pwn","state":"ok","detail":"","failed_units":[],"job_account":{"name":"runner","uid":2000}}"#,
        br#"{"schema":1,"contract":2,"ready":true,"gate":"ready","state":"ok","detail":"","failed_units":[],"job_account":{"name":"runner","uid":2000},"extra":1}"#,
        // A schema or contract this daemon does not speak.
        br#"{"schema":2,"contract":2,"ready":true,"gate":"ready","state":"ok","detail":"","failed_units":[],"job_account":{"name":"runner","uid":2000}}"#,
        br#"{"schema":1,"contract":1,"ready":true,"gate":"ready","state":"ok","detail":"","failed_units":[],"job_account":{"name":"runner","uid":2000}}"#,
        // More failed units than the helper promises, and a unit name that is
        // not one.
        br#"{"schema":1,"contract":2,"ready":false,"gate":"units","state":"inactive","detail":"","failed_units":["a","b","c","d","e","f","g","h","i"],"job_account":{"name":"runner","uid":2000}}"#,
        br#"{"schema":1,"contract":2,"ready":false,"gate":"units","state":"inactive","detail":"","failed_units":["a b; rm -rf /"],"job_account":{"name":"runner","uid":2000}}"#,
    ];
    for report in refused {
        assert!(
            GuestReadiness::parse(report, 2).is_err(),
            "accepted {}",
            String::from_utf8_lossy(report)
        );
    }

    let oversized = format!(
        r#"{{"schema":1,"contract":2,"ready":true,"gate":"ready","state":"ok",
        "detail":"{}","failed_units":[],"job_account":{{"name":"runner","uid":2000}}}}"#,
        "a".repeat(4_096)
    );
    assert!(GuestReadiness::parse(oversized.as_bytes(), 2).is_err());
}

fn helper_config() -> flanforge_libvirt_wire::HelperConfig {
    flanforge_libvirt_wire::HelperConfig {
        uri: "qemu:///system".to_owned(),
        pool: "flanforge".to_owned(),
        network: "flanforge-ci".to_owned(),
        state_dir: "/var/lib/flanforge".into(),
        service_instance: Uuid::new_v4(),
        min_storage_free_bytes: 1,
        allow_insecure_transport: false,
        is_warm_declared: false,
    }
}

fn ownership_manifest() -> flanforge_libvirt_wire::OwnershipManifest {
    flanforge_libvirt_wire::OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "flanforge-allocation".to_owned(),
        Uuid::new_v4(),
        "52:54:00:00:00:01".to_owned(),
        flanforge_libvirt_wire::Artifact::new("allocation.root.qcow2".to_owned()),
        flanforge_libvirt_wire::Artifact::new("allocation.seed.img".to_owned()),
        "flanforge-allocation".to_owned(),
        std::path::PathBuf::from("/var/lib/flanforge/allocations/one/known_hosts"),
        1,
    )
}

/// A guest login shell may print before the command runs, which is why the
/// staging reader already takes the last line. The report is read the same way.
#[test]
fn a_login_shell_banner_before_the_report_does_not_break_the_parse() {
    let noisy = b"Last login: Tue\n\n{\"schema\":1,\"contract\":2,\"ready\":true,\
        \"gate\":\"ready\",\"state\":\"ok\",\"detail\":\"\",\"failed_units\":[],\
        \"job_account\":{\"name\":\"runner\",\"uid\":2000}}\n";
    let report =
        GuestReadiness::parse(noisy, 2).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(report.is_ready());
}

/// A program form given stdin must reach the helper so it is refused there,
/// rather than being dropped into a guest whose `read` never returns.
#[test]
fn a_program_form_carries_stdin_to_the_refusal_rather_than_dropping_it() {
    let arguments = ["--wait-seconds".to_owned(), "20".to_owned()];
    let request = program(
        "/usr/local/libexec/flanforge-guest-ready",
        &arguments,
        Some(b"secret\n".to_vec()),
        false,
    );
    assert!(request.input.is_some());
    assert!(
        HelperRequest::AgentExec {
            config: helper_config(),
            manifest: ownership_manifest(),
            request,
            timeout_seconds: 5,
        }
        .encode()
        .is_err(),
        "the helper accepted stdin on a shape that reads none"
    );
}
