use serde::Deserialize;

use flanforge_manager::WorkerError;

/// The helper emits one bounded JSON line by construction; the cap is applied
/// before parsing so a guest that ignored its own contract cannot be the one
/// deciding how much work the daemon does.
pub(super) const MAX_REPORT_BYTES: usize = 4_096;

const MAX_STATE_BYTES: usize = 64;
const MAX_DETAIL_BYTES: usize = 256;
const MAX_FAILED_UNITS: usize = 8;
const MAX_UNIT_BYTES: usize = 64;
const READY_SCHEMA: u8 = 1;

/// The gates the baked helper can stop at. Closed, so an unrecognised gate is a
/// contract mismatch rather than a string the daemon repeats.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReadyGate {
    Account,
    SelfTest,
    CloudInit,
    Units,
    Session,
    Runner,
    Ready,
}

impl std::fmt::Display for ReadyGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Account => "account",
            Self::SelfTest => "self-test",
            Self::CloudInit => "cloud-init",
            Self::Units => "units",
            Self::Session => "session",
            Self::Runner => "runner",
            Self::Ready => "ready",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema: u8,
    contract: u8,
    ready: bool,
    gate: ReadyGate,
    state: String,
    detail: String,
    failed_units: Vec<String>,
    job_account: ReportedAccount,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedAccount {
    name: String,
    uid: u32,
}

/// What one readiness probe said, once every field has been re-bounded on this
/// side of the channel. The helper bounds its own output; validating again here
/// is what makes the guest's bytes data rather than trust.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuestReadiness {
    is_ready: bool,
    gate: ReadyGate,
    state: String,
    detail: String,
    failed_units: Vec<String>,
    account_name: String,
    account_uid: u32,
}

impl GuestReadiness {
    /// # Errors
    /// Returns an error for an oversized, malformed, or out-of-contract report.
    pub(crate) fn parse(bytes: &[u8], contract_version: u8) -> Result<Self, WorkerError> {
        if bytes.is_empty() || bytes.len() > MAX_REPORT_BYTES {
            return Err(WorkerError::new("guest readiness report size is invalid"));
        }
        // A guest login shell may print before the command runs, so the report
        // is the last non-empty line rather than the whole capture.
        let line = bytes
            .split(|byte| *byte == b'\n')
            .rev()
            .find(|line| !line.iter().all(u8::is_ascii_whitespace))
            .unwrap_or_default();
        let report: Report = serde_json::from_slice(line)
            .map_err(|_| WorkerError::new("guest readiness report is malformed"))?;
        if report.schema != READY_SCHEMA || report.contract != contract_version {
            return Err(WorkerError::new(
                "guest readiness report does not match the image contract",
            ));
        }
        if !is_bounded_text(&report.state, MAX_STATE_BYTES)
            || !is_bounded_text(&report.detail, MAX_DETAIL_BYTES)
            || report.failed_units.len() > MAX_FAILED_UNITS
            || report.failed_units.iter().any(|unit| !is_unit_name(unit))
        {
            return Err(WorkerError::new(
                "guest readiness report is not within its own bounds",
            ));
        }
        Ok(Self {
            is_ready: report.ready,
            gate: report.gate,
            state: report.state,
            detail: report.detail,
            failed_units: report.failed_units,
            account_name: report.job_account.name,
            account_uid: report.job_account.uid,
        })
    }

    pub(crate) const fn is_ready(&self) -> bool {
        self.is_ready
    }

    pub(crate) const fn gate(&self) -> ReadyGate {
        self.gate
    }

    pub(crate) fn account(&self) -> (&str, u32) {
        (&self.account_name, self.account_uid)
    }

    /// The operator-facing cause, with the gate that produced it. A named cause
    /// in seconds is the whole point of this probe over a boot timeout.
    pub(crate) fn cause(&self) -> String {
        let units = if self.failed_units.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.failed_units.join(", "))
        };
        format!(
            "guest provisioning failed at gate {} ({}): {}{units}",
            self.gate, self.state, self.detail
        )
    }
}

fn is_bounded_text(value: &str, maximum: usize) -> bool {
    value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
}

fn is_unit_name(value: &str) -> bool {
    (1..=MAX_UNIT_BYTES).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'@' | b'.' | b'_' | b'-'))
}
