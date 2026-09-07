use super::{RECYCLE_CONTRACT_VERSION, RecycleGate, RecycleVerdict};

/// The verdict is one line by contract, so the fixture is too.
const CLEAN: &[u8] = br#"{"schema":1,"contract":1,"clean":true,"gate":"clean","state":"apps","detail":"","free_mb":20000,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#;

#[test]
fn a_clean_verdict_carries_what_the_operator_needs() {
    let verdict = RecycleVerdict::parse(CLEAN, RECYCLE_CONTRACT_VERSION)
        .unwrap_or_else(|error| unreachable!("clean verdict: {error}"));
    assert!(verdict.is_clean());
    assert_eq!(verdict.free_mb(), 20_000);
    assert_eq!(verdict.skew_seconds(), 0);
}

/// A guest login shell may print before the command runs, so the verdict is
/// the last non-empty line rather than the whole capture.
#[test]
fn a_login_banner_before_the_verdict_is_ignored() {
    let mut bytes = b"Last login: Tue\n\n".to_vec();
    bytes.extend_from_slice(CLEAN);
    bytes.push(b'\n');
    let verdict = RecycleVerdict::parse(&bytes, RECYCLE_CONTRACT_VERSION)
        .unwrap_or_else(|error| unreachable!("banner: {error}"));
    assert!(verdict.is_clean());
}

/// The one shape a compromised guest would most want to send: a pass it did
/// not earn. A clean claim at a gate the reset never reached is refused.
#[test]
fn a_clean_claim_at_an_unreached_gate_is_refused() {
    let forged = br#"{"schema":1,"contract":1,"clean":true,"gate":"processes","state":"surviving","detail":"","free_mb":1,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#;
    assert!(RecycleVerdict::parse(forged, RECYCLE_CONTRACT_VERSION).is_err());
}

#[test]
fn a_verdict_outside_the_contract_or_its_own_bounds_is_refused() {
    for report in [
        // A different image contract than this daemon speaks.
        br#"{"schema":1,"contract":2,"clean":true,"gate":"clean","state":"","detail":"","free_mb":1,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#.as_slice(),
        // A schema this build does not know.
        br#"{"schema":9,"contract":1,"clean":true,"gate":"clean","state":"","detail":"","free_mb":1,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#,
        // A gate name no version of the helper emits.
        br#"{"schema":1,"contract":1,"clean":false,"gate":"invented","state":"","detail":"","free_mb":1,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#,
        // A field the contract does not carry.
        br#"{"schema":1,"contract":1,"clean":true,"gate":"clean","state":"","detail":"","free_mb":1,"skew_seconds":0,"extra":1,"job_account":{"name":"runner","uid":2000}}"#,
        b"",
        b"not json at all",
    ] {
        assert!(
            RecycleVerdict::parse(report, RECYCLE_CONTRACT_VERSION).is_err(),
            "{}",
            String::from_utf8_lossy(report)
        );
    }
}

/// The detail is written by a machine a previous job could have influenced, so
/// an oversized or non-printable one is refused rather than logged.
#[test]
fn an_unbounded_detail_is_refused_rather_than_carried() {
    let hostile = "a".repeat(257);
    let report = format!(
        r#"{{"schema":1,"contract":1,"clean":false,"gate":"disk","state":"full","detail":"{hostile}","free_mb":0,"skew_seconds":0,"job_account":{{"name":"runner","uid":2000}}}}"#
    );
    assert!(RecycleVerdict::parse(report.as_bytes(), RECYCLE_CONTRACT_VERSION).is_err());
}

#[test]
fn a_failed_gate_names_itself_in_one_line() {
    let report = br#"{"schema":1,"contract":1,"clean":false,"gate":"processes","state":"surviving","detail":"4242 leftover-daemon","free_mb":9,"skew_seconds":0,"job_account":{"name":"runner","uid":2000}}"#;
    let verdict = RecycleVerdict::parse(report, RECYCLE_CONTRACT_VERSION)
        .unwrap_or_else(|error| unreachable!("failed gate: {error}"));
    assert!(!verdict.is_clean());
    assert_eq!(
        verdict.cause(),
        "the recycle gate failed at processes (surviving): 4242 leftover-daemon"
    );
    assert_eq!(RecycleGate::Processes, RecycleGate::Processes);
}
