use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};

use uuid::Uuid;

use super::{HelperConfig, HelperFailureCode, HelperReply, HelperRequest};
use crate::{Artifact, BaseImageManifest};

const TEMPLATE_FIXTURE: &[u8] =
    include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json");

fn config() -> HelperConfig {
    HelperConfig {
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

/// The sweep can only act on a machine whose age it knows, and inventory
/// always crosses this boundary. Dropping the age here made every libvirt
/// sweep plan nothing and left reclamation unreachable.
#[test]
fn inventory_carries_machine_age_across_the_helper_boundary() {
    let reply = HelperReply::Inventory(vec![crate::HelperMachine {
        name: "ci-project-42-1".to_owned(),
        state: crate::HelperMachineState::Running,
        age_seconds: Some(4_242),
        size: None,
        ownership: crate::HelperMachineOwnership::Owned,
    }]);
    let encoded = reply
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let decoded =
        HelperReply::parse(&encoded).unwrap_or_else(|error| unreachable!("parse: {error}"));
    let HelperReply::Inventory(machines) = decoded else {
        unreachable!("inventory reply")
    };
    assert_eq!(
        machines.first().and_then(|machine| machine.age_seconds),
        Some(4_242)
    );
}

#[test]
fn helper_frames_are_bounded_and_validate_configuration() {
    let request = HelperRequest::Probe { config: config() };
    let encoded = request
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert!(HelperRequest::parse(&encoded).is_ok());

    // A remote host is a supported topology; clear-text tcp still is not,
    // unless the operator has said so on this very config.
    let remote = HelperRequest::Probe {
        config: HelperConfig {
            uri: "qemu+ssh://foreign/system".to_owned(),
            ..config()
        },
    };
    assert!(remote.encode().is_ok());

    let invalid = HelperRequest::Probe {
        config: HelperConfig {
            uri: "qemu+tcp://foreign/system".to_owned(),
            ..config()
        },
    };
    assert!(invalid.encode().is_err());

    let permitted = HelperRequest::Probe {
        config: HelperConfig {
            uri: "qemu+tcp://foreign/system".to_owned(),
            allow_insecure_transport: true,
            ..config()
        },
    };
    assert!(permitted.encode().is_ok());

    for state_dir in [
        PathBuf::from("relative/state"),
        PathBuf::from("/var/lib/flanforge/./state"),
        PathBuf::from(format!("/{}", "a".repeat(4_096))),
    ] {
        let invalid = HelperRequest::Probe {
            config: HelperConfig {
                state_dir,
                ..config()
            },
        };
        assert!(invalid.encode().is_err());
    }
}

#[test]
fn base_check_requires_a_structurally_valid_exact_publication() {
    let manifest = BaseImageManifest::parse(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let publication = crate::PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "flanforge-base-aabbcc.qcow2".to_owned(),
        "/pool/flanforge-base-aabbcc.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("publication: {error}"));
    let pointer = publication
        .pointer()
        .unwrap_or_else(|error| unreachable!("pointer: {error}"));
    assert!(
        HelperRequest::CheckSource {
            config: config(),
            pointer,
        }
        .encode()
        .is_ok()
    );
}

#[test]
fn helper_import_is_bound_to_the_generated_name_and_staging_path() {
    let import_id = Uuid::new_v4();
    let volume_name = format!("base-import-{import_id}.qcow2");
    let staged_image_path = PathBuf::from(format!(
        "/var/lib/flanforge/libvirt/imports/import-{import_id}.qcow2"
    ));
    let manifest = BaseImageManifest::parse(TEMPLATE_FIXTURE)
        .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let valid = HelperRequest::Import {
        config: config(),
        logical_name: "flanforge-base".to_owned(),
        volume_name: volume_name.clone(),
        staged_image_path: staged_image_path.clone(),
        manifest: manifest.clone(),
    };
    assert!(valid.encode().is_ok());

    for invalid_path in [
        PathBuf::from(format!(
            "/var/lib/flanforge/libvirt/imports/import-{}.qcow2",
            Uuid::new_v4()
        )),
        PathBuf::from(format!(
            "/var/lib/flanforge/./libvirt/imports/import-{import_id}.qcow2"
        )),
    ] {
        let invalid = HelperRequest::Import {
            config: config(),
            logical_name: "flanforge-base".to_owned(),
            volume_name: volume_name.clone(),
            staged_image_path: invalid_path,
            manifest: manifest.clone(),
        };
        assert!(invalid.encode().is_err());
    }

    let invalid = HelperRequest::Import {
        config: config(),
        logical_name: "flanforge-base".to_owned(),
        volume_name: format!("base-import-{}.qcow2", Uuid::new_v4()),
        staged_image_path,
        manifest,
    };
    assert!(invalid.encode().is_err());
}

#[test]
fn helper_delete_volume_requires_an_exact_safe_key() {
    let valid = HelperRequest::DeleteVolume {
        config: config(),
        key: Some("/var/lib/libvirt/images/root.qcow2".to_owned()),
        name: "root.qcow2".to_owned(),
    };
    assert!(valid.encode().is_ok());
    let opaque = HelperRequest::DeleteVolume {
        config: config(),
        key: Some("rbd://pool/root.qcow2".to_owned()),
        name: "root.qcow2".to_owned(),
    };
    assert!(opaque.encode().is_ok());

    for key in ["", ".", "../foreign", "/pool/./foreign", "/pool//foreign"] {
        let invalid = HelperRequest::DeleteVolume {
            config: config(),
            key: Some(key.to_owned()),
            name: "root.qcow2".to_owned(),
        };
        assert!(invalid.encode().is_err(), "accepted {key}");
    }
}

#[test]
fn helper_errors_are_sanitized_before_crossing_the_process_boundary() {
    let reply = HelperReply::error(
        HelperFailureCode::Ownership,
        format!("first\nsecond\0{}", "é".repeat(400)),
    );
    let encoded = reply
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let decoded =
        HelperReply::parse(&encoded).unwrap_or_else(|error| unreachable!("parse: {error}"));
    let HelperReply::Error(failure) = decoded else {
        unreachable!("error reply")
    };
    assert_eq!(failure.code(), HelperFailureCode::Ownership);
    assert!(!failure.message().contains(['\n', '\0']));
    assert!(failure.message().len() <= super::MAX_LIBVIRT_HELPER_FAILURE_BYTES);
}

#[test]
fn helper_rejects_timeouts_outside_the_bounded_contract() {
    let manifest = crate::OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "flanforge-allocation".to_owned(),
        Uuid::new_v4(),
        "52:54:00:00:00:01".to_owned(),
        Artifact::new("allocation.root.qcow2".to_owned()),
        Artifact::new("allocation.seed.img".to_owned()),
        "flanforge-allocation".to_owned(),
        PathBuf::from("/var/lib/flanforge/allocations/allocation/known_hosts"),
        1,
    );
    let request = HelperRequest::Address {
        config: config(),
        manifest,
        timeout_seconds: 0,
    };
    assert!(request.encode().is_err());
}

#[test]
fn helper_reply_round_trips_an_address() {
    let reply = HelperReply::Address(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    let encoded = reply
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert!(matches!(
        HelperReply::parse(&encoded),
        Ok(HelperReply::Address(Some(address))) if address == IpAddr::V4(Ipv4Addr::LOCALHOST)
    ));
}

fn ownership_manifest() -> crate::OwnershipManifest {
    crate::OwnershipManifest::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "flanforge-allocation".to_owned(),
        Uuid::new_v4(),
        "52:54:00:00:00:01".to_owned(),
        Artifact::new("allocation.root.qcow2".to_owned()),
        Artifact::new("allocation.seed.img".to_owned()),
        "flanforge-allocation".to_owned(),
        PathBuf::from("/var/lib/flanforge/allocations/allocation/known_hosts"),
        1,
    )
}

fn exec(path: &str, arguments: &[&str]) -> super::AgentExecRequest {
    super::AgentExecRequest {
        path: path.to_owned(),
        arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
        input: None,
        is_output_captured: false,
    }
}

fn agent_exec(request: super::AgentExecRequest) -> HelperRequest {
    HelperRequest::AgentExec {
        config: config(),
        manifest: ownership_manifest(),
        request,
        timeout_seconds: 5,
    }
}

/// The helper runs with libvirt's authority, so the four shapes are the whole
/// contract: an allow-list on argv[0] alone would admit `runuser -l root`.
#[test]
fn the_agent_accepts_exactly_the_four_argv_shapes_it_needs() {
    let unit = format!("flanforge-job-{}.service", Uuid::new_v4());
    let accepted = [
        exec("/usr/sbin/runuser", &["-l", "runner", "-c", "echo hello"]),
        exec(
            "/usr/bin/systemd-run",
            &[
                &format!("--unit={unit}"),
                "--pipe",
                "--wait",
                "--property=RuntimeMaxSec=3600",
                "--property=TimeoutStopSec=10",
                "/usr/sbin/runuser",
                "-l",
                "runner",
                "-c",
                "echo hello",
            ],
        ),
        exec("/usr/bin/systemctl", &["stop", &unit]),
        exec(
            "/usr/local/libexec/flanforge-guest-ready",
            &["--wait-seconds", "20"],
        ),
    ];
    for request in accepted {
        let path = request.path.clone();
        assert!(agent_exec(request).encode().is_ok(), "refused {path}");
    }
}

#[test]
fn the_agent_refuses_every_other_executor_shape() {
    let unit = format!("flanforge-job-{}.service", Uuid::new_v4());
    let other = format!("flanforge-job-{}.service", Uuid::new_v4());
    let refused = [
        // A generic executor an argv[0] allow-list would have admitted.
        exec("/usr/sbin/runuser", &["-l", "root", "-c", "id"]),
        exec("/usr/sbin/runuser", &["-l", "-runner", "-c", "id"]),
        exec("/usr/sbin/runuser", &["-u", "runner", "--", "/bin/sh"]),
        exec("/usr/bin/systemd-run", &["/bin/sh", "-c", "id"]),
        exec("/usr/bin/systemctl", &["start", &unit]),
        exec("/usr/bin/systemctl", &["stop", "sshd.service"]),
        exec("/usr/bin/systemctl", &["stop", "flanforge-job-.service"]),
        exec("/bin/sh", &["-c", "id"]),
        exec("/usr/local/libexec/flanforge-guest-ready", &["--self-test"]),
        exec(
            "/usr/local/libexec/flanforge-guest-ready",
            &["--wait-seconds", "0"],
        ),
        exec(
            "/usr/local/libexec/flanforge-guest-ready",
            &["--wait-seconds", "121"],
        ),
        exec(
            "/usr/local/libexec/flanforge-guest-ready",
            &["--wait-seconds", "+5"],
        ),
        // A unit name whose UUID does not rebuild to the name that was sent.
        exec(
            "/usr/bin/systemd-run",
            &[
                &format!("--unit={other}x"),
                "--pipe",
                "--wait",
                "--property=RuntimeMaxSec=60",
                "--property=TimeoutStopSec=10",
                "/usr/sbin/runuser",
                "-l",
                "runner",
                "-c",
                "id",
            ],
        ),
        // A stop budget systemd would never escalate from.
        exec(
            "/usr/bin/systemd-run",
            &[
                &format!("--unit={unit}"),
                "--pipe",
                "--wait",
                "--property=RuntimeMaxSec=0",
                "--property=TimeoutStopSec=10",
                "/usr/sbin/runuser",
                "-l",
                "runner",
                "-c",
                "id",
            ],
        ),
        exec(
            "/usr/bin/systemd-run",
            &[
                &format!("--unit={unit}"),
                "--pipe",
                "--wait",
                "--property=RuntimeMaxSec=60",
                "--property=TimeoutStopSec=0",
                "/usr/sbin/runuser",
                "-l",
                "runner",
                "-c",
                "id",
            ],
        ),
    ];
    for request in refused {
        let shape = format!("{} {:?}", request.path, request.arguments);
        assert!(agent_exec(request).encode().is_err(), "accepted {shape}");
    }
}

#[test]
fn agent_input_and_capture_are_bound_to_the_shapes_that_may_carry_them() {
    let unit = format!("flanforge-job-{}.service", Uuid::new_v4());
    let with_input = |mut request: super::AgentExecRequest, input: Vec<u8>| {
        request.input = Some(input);
        request
    };
    let secret = b"token\n".to_vec();
    assert!(
        agent_exec(with_input(
            exec("/usr/sbin/runuser", &["-l", "runner", "-c", "read x"]),
            secret.clone()
        ))
        .encode()
        .is_ok()
    );
    // stdin on a command that takes none, and capture on one carrying a secret.
    assert!(
        agent_exec(with_input(
            exec("/usr/bin/systemctl", &["stop", &unit]),
            secret.clone()
        ))
        .encode()
        .is_err()
    );
    let mut capturing = with_input(
        exec("/usr/sbin/runuser", &["-l", "runner", "-c", "read x"]),
        secret,
    );
    capturing.is_output_captured = true;
    assert!(agent_exec(capturing).encode().is_err());

    let mut oversized = exec("/usr/sbin/runuser", &["-l", "runner", "-c", "read x"]);
    oversized.input = Some(vec![b'a'; 64 * 1_024 + 1]);
    assert!(agent_exec(oversized).encode().is_err());

    let mut empty = exec("/usr/sbin/runuser", &["-l", "runner", "-c", "read x"]);
    empty.input = Some(Vec::new());
    assert!(agent_exec(empty).encode().is_err());
}

/// The token is one `tracing::debug!` away from a log otherwise, because
/// `HelperRequest` derives `Debug`.
#[test]
fn an_agent_exec_never_prints_its_input() {
    let mut request = exec("/usr/sbin/runuser", &["-l", "runner", "-c", "read x"]);
    request.input = Some(b"super-secret-token\n".to_vec());
    let rendered = format!("{:?}", agent_exec(request));
    assert!(!rendered.contains("super-secret-token"));
    assert!(rendered.contains("input_bytes: 19"));
}

#[test]
fn an_agent_status_request_and_its_outcome_are_bounded() {
    for (pid, timeout) in [(0_i64, 5_u8), (-1, 5), (1_234, 0), (1_234, 6)] {
        assert!(
            HelperRequest::AgentExecStatus {
                config: config(),
                manifest: ownership_manifest(),
                pid,
                timeout_seconds: timeout,
            }
            .encode()
            .is_err(),
            "accepted pid {pid} timeout {timeout}"
        );
    }
    assert!(
        HelperRequest::AgentExecStatus {
            config: config(),
            manifest: ownership_manifest(),
            pid: 1_234,
            timeout_seconds: 5,
        }
        .encode()
        .is_ok()
    );

    assert!(HelperReply::AgentStarted(0).encode().is_err());
    assert!(HelperReply::AgentStarted(1_234).encode().is_ok());
    assert!(
        HelperReply::AgentOutcome(super::AgentExecOutcome::Running)
            .encode()
            .is_ok()
    );
    // A finished process carries exactly one verdict, never both and never
    // neither, and a lost observation carries a bounded printable reason.
    for outcome in [
        super::AgentExecOutcome::Exited {
            exit_code: Some(0),
            signal: Some(9),
            stdout: Vec::new(),
            is_truncated: false,
            stderr: Vec::new(),
        },
        super::AgentExecOutcome::Exited {
            exit_code: None,
            signal: None,
            stdout: Vec::new(),
            is_truncated: false,
            stderr: Vec::new(),
        },
        super::AgentExecOutcome::Exited {
            exit_code: Some(0),
            signal: None,
            stdout: vec![b'a'; 16 * 1_024 + 1],
            is_truncated: false,
            stderr: Vec::new(),
        },
        super::AgentExecOutcome::Lost {
            reason: String::new(),
        },
        super::AgentExecOutcome::Lost {
            reason: "line\nbreak".to_owned(),
        },
    ] {
        assert!(
            HelperReply::AgentOutcome(outcome.clone()).encode().is_err(),
            "accepted {outcome:?}"
        );
    }
    let exited = super::AgentExecOutcome::Exited {
        exit_code: Some(7),
        signal: None,
        stdout: b"{}".to_vec(),
        is_truncated: false,
        stderr: Vec::new(),
    };
    let encoded = HelperReply::AgentOutcome(exited.clone())
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert!(matches!(
        HelperReply::parse(&encoded),
        Ok(HelperReply::AgentOutcome(decoded)) if decoded == exited
    ));
}
