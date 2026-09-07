use flanforge_libvirt_wire::AgentExecOutcome;

use super::{ensure_ping_answered, exec_capability, exec_outcome, exec_pid};

#[test]
fn a_ping_is_answered_only_by_an_empty_result() {
    assert!(ensure_ping_answered(r#"{"return":{}}"#).is_ok());
    for refused in [
        "",
        "{}",
        r#"{"return":null}"#,
        r#"{"error":{"class":"CommandNotFound","desc":"blocked"}}"#,
        "not json",
    ] {
        assert!(ensure_ping_answered(refused).is_err(), "accepted {refused}");
    }
}

/// A blocked RPC is a fact about the image, so it is read from the agent's own
/// capability list rather than inferred from an error string.
#[test]
fn the_capability_list_reports_a_blocked_rpc_without_running_anything() {
    let enabled = r#"{"return":{"version":"9.1.0","supported_commands":[
        {"name":"guest-exec","enabled":true,"success-response":true},
        {"name":"guest-file-open","enabled":false,"success-response":true}]}}"#;
    assert_eq!(exec_capability(enabled, "guest-exec"), Ok(true));
    assert_eq!(exec_capability(enabled, "guest-file-open"), Ok(false));
    assert_eq!(exec_capability(enabled, "guest-exec-status"), Ok(false));

    let blocked = r#"{"return":{"supported_commands":[
        {"name":"guest-exec","enabled":false,"success-response":true}]}}"#;
    assert_eq!(exec_capability(blocked, "guest-exec"), Ok(false));

    let commands = (0..257)
        .map(|index| format!(r#"{{"name":"c{index}","enabled":true}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert!(
        exec_capability(
            &format!(r#"{{"return":{{"supported_commands":[{commands}]}}}}"#),
            "c1"
        )
        .is_err()
    );
}

#[test]
fn a_started_command_reports_one_usable_pid() {
    assert_eq!(exec_pid(r#"{"return":{"pid":1234}}"#), Ok(1_234));
    for refused in [
        r#"{"return":{"pid":0}}"#,
        r#"{"return":{"pid":-1}}"#,
        r#"{"return":{}}"#,
        r#"{"error":{"class":"CommandNotFound","desc":"guest-exec has been disabled"}}"#,
    ] {
        assert!(exec_pid(refused).is_err(), "accepted {refused}");
    }
}

#[test]
fn a_status_reply_distinguishes_running_from_a_verdict() {
    assert_eq!(
        exec_outcome(r#"{"return":{"exited":false}}"#),
        Ok(AgentExecOutcome::Running)
    );
    assert_eq!(
        exec_outcome(r#"{"return":{"exited":true,"exitcode":7}}"#),
        Ok(AgentExecOutcome::Exited {
            exit_code: Some(7),
            signal: None,
            stdout: Vec::new(),
            is_truncated: false,
            stderr: Vec::new(),
        })
    );
    assert_eq!(
        exec_outcome(r#"{"return":{"exited":true,"signal":9}}"#),
        Ok(AgentExecOutcome::Exited {
            exit_code: None,
            signal: Some(9),
            stdout: Vec::new(),
            is_truncated: false,
            stderr: Vec::new(),
        })
    );
    // Neither verdict and both verdicts are equally uninterpretable.
    assert!(exec_outcome(r#"{"return":{"exited":true}}"#).is_err());
    assert!(exec_outcome(r#"{"return":{"exited":true,"exitcode":0,"signal":9}}"#).is_err());
}

/// The rule is deliberately independent of the agent's wording: if the agent
/// answers with any in-band error, it can no longer speak for this pid.
#[test]
fn an_in_band_status_error_is_a_lost_observation_and_never_a_verdict() {
    let outcome =
        exec_outcome(r#"{"error":{"class":"GenericError","desc":"PID 1234 does not exist"}}"#);
    assert_eq!(
        outcome,
        Ok(AgentExecOutcome::Lost {
            reason: "GenericError: PID 1234 does not exist".to_owned(),
        })
    );
}

#[test]
fn captured_output_is_base64_decoded_and_bounded() {
    let decoded = exec_outcome(r#"{"return":{"exited":true,"exitcode":0,"out-data":"aGk="}}"#);
    assert_eq!(
        decoded,
        Ok(AgentExecOutcome::Exited {
            exit_code: Some(0),
            signal: None,
            stdout: b"hi".to_vec(),
            is_truncated: false,
            stderr: Vec::new(),
        })
    );
    assert!(exec_outcome(r#"{"return":{"exited":true,"exitcode":0,"out-data":"!!"}}"#).is_err());

    // The agent's own truncation flag is carried through rather than hidden.
    let truncated = exec_outcome(
        r#"{"return":{"exited":true,"exitcode":0,"out-data":"aGk=","out-truncated":true}}"#,
    );
    assert!(matches!(
        truncated,
        Ok(AgentExecOutcome::Exited {
            is_truncated: true,
            ..
        })
    ));
}

/// A reply larger than the envelope is refused whole. Capture is enabled on one
/// self-bounding command precisely because an oversized reply would lose the
/// exit code for good.
#[test]
fn an_oversized_or_malformed_reply_is_refused_rather_than_truncated() {
    let oversized = format!(
        r#"{{"return":{{"exited":false,"pad":"{}"}}}}"#,
        "a".repeat(70_000)
    );
    assert!(exec_outcome(&oversized).is_err());
    assert!(exec_outcome("").is_err());
    assert!(exec_outcome("[]").is_err());
    let long_error = format!(
        r#"{{"error":{{"class":"GenericError","desc":"{}"}}}}"#,
        "a".repeat(300)
    );
    assert!(exec_outcome(&long_error).is_err());
}
