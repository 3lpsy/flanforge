use flanforge_manager::WorkerError;
use serde::Deserialize;

/// The baked recycle gate, at the same absolute path on both platforms so the
/// daemon runs one named command and knows nothing about either process model.
pub const RECYCLE_HELPER: &str = "/usr/local/libexec/flanforge-guest-recycle";

/// The gate contract this daemon speaks. The gate reports its own, and a
/// mismatch is a refusal rather than a best-effort read of unknown fields.
pub const RECYCLE_CONTRACT_VERSION: u8 = 1;

/// The whole verdict is one bounded line, so a guest cannot grow the daemon's
/// memory with its stdout.
pub const RECYCLE_MAX_REPORT_BYTES: usize = 4_096;

const RECYCLE_SCHEMA: u8 = 1;
const MAX_STATE_BYTES: usize = 64;
const MAX_DETAIL_BYTES: usize = 256;

/// Which gate the reset stopped at. A closed set, so a value this build does
/// not know is a refusal rather than a silently accepted pass.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RecycleGate {
    Account,
    SelfTest,
    Processes,
    Clock,
    Disk,
    Runner,
    Clean,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema: u8,
    contract: u8,
    clean: bool,
    gate: RecycleGate,
    state: String,
    detail: String,
    free_mb: u64,
    skew_seconds: u64,
    job_account: ReportedAccount,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedAccount {
    name: String,
    uid: u32,
}

/// One recycle gate's verdict, re-bounded host-side. Every field the guest
/// wrote is checked again here: the guest is the one thing a hot machine's
/// previous job could have influenced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecycleVerdict {
    is_clean: bool,
    gate: RecycleGate,
    state: String,
    detail: String,
    free_mb: u64,
    skew_seconds: u64,
    account_name: String,
    account_uid: u32,
}

impl RecycleVerdict {
    /// Parses one bounded JSON line.
    ///
    /// # Errors
    ///
    /// Returns an error when the report is absent, oversized, malformed, does
    /// not match the image contract, or is outside its own stated bounds.
    pub fn parse(bytes: &[u8], contract_version: u8) -> Result<Self, WorkerError> {
        if bytes.is_empty() || bytes.len() > RECYCLE_MAX_REPORT_BYTES {
            return Err(WorkerError::new("recycle verdict size is invalid"));
        }
        // A guest login shell may print before the command runs, so the report
        // is the last non-empty line rather than the whole capture.
        let line = bytes
            .split(|byte| *byte == b'\n')
            .rev()
            .find(|line| !line.iter().all(u8::is_ascii_whitespace))
            .unwrap_or_default();
        let report: Report = serde_json::from_slice(line)
            .map_err(|_| WorkerError::new("recycle verdict is malformed"))?;
        if report.schema != RECYCLE_SCHEMA || report.contract != contract_version {
            return Err(WorkerError::new(
                "recycle verdict does not match the image contract",
            ));
        }
        if !is_bounded_text(&report.state, MAX_STATE_BYTES)
            || !is_bounded_text(&report.detail, MAX_DETAIL_BYTES)
            || !is_bounded_text(&report.job_account.name, MAX_STATE_BYTES)
        {
            return Err(WorkerError::new(
                "recycle verdict is not within its own bounds",
            ));
        }
        // A clean verdict that names any gate but `clean` is incoherent, and
        // the safe reading of an incoherent verdict is "not clean".
        if report.clean && report.gate != RecycleGate::Clean {
            return Err(WorkerError::new(
                "recycle verdict claims a clean machine at a gate it did not reach",
            ));
        }
        Ok(Self {
            is_clean: report.clean,
            gate: report.gate,
            state: report.state,
            detail: report.detail,
            free_mb: report.free_mb,
            skew_seconds: report.skew_seconds,
            account_name: report.job_account.name,
            account_uid: report.job_account.uid,
        })
    }

    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.is_clean
    }

    #[must_use]
    pub const fn free_mb(&self) -> u64 {
        self.free_mb
    }

    #[must_use]
    pub const fn skew_seconds(&self) -> u64 {
        self.skew_seconds
    }

    /// The account the gate reset, as the image reports it. Logged rather than
    /// enforced: the gate already refuses a missing or privileged account
    /// internally, so this is for the operator reading why a machine rejoined
    /// the pool, not a second gate.
    #[must_use]
    pub fn account(&self) -> (&str, u32) {
        (&self.account_name, self.account_uid)
    }

    /// One operator-facing line naming where the reset stopped. Every byte in
    /// it was bounded above, so it is safe to log and to carry into an error.
    #[must_use]
    pub fn cause(&self) -> String {
        let gate = format!("{:?}", self.gate).to_lowercase();
        if self.detail.is_empty() {
            format!("the recycle gate failed at {gate} ({})", self.state)
        } else {
            format!(
                "the recycle gate failed at {gate} ({}): {}",
                self.state, self.detail
            )
        }
    }
}

fn is_bounded_text(value: &str, limit: usize) -> bool {
    value.len() <= limit
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}
