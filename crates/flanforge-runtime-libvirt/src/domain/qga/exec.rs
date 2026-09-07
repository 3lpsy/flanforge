use base64::{Engine, engine::general_purpose::STANDARD};
use flanforge_libvirt_wire::AgentExecOutcome;
use serde::Deserialize;

use crate::RuntimeError;

use super::envelope::{Answer, answer, decode};

/// The agent frees its `GuestExecInfo` on the first reply reporting `exited`,
/// so the exit code is gone once read. Anything that would grow the reply past
/// the envelope bound therefore loses the exit code permanently: never enable
/// `capture-output` on the job, retention gates, or the Tailscale join.
const MAX_CAPTURED_BYTES: usize = 16 * 1_024;

#[derive(Deserialize)]
struct Started {
    pid: i64,
}

#[derive(Deserialize)]
struct Status {
    exited: bool,
    exitcode: Option<i32>,
    signal: Option<i32>,
    #[serde(rename = "out-data")]
    out_data: Option<String>,
    #[serde(rename = "out-truncated", default)]
    is_out_truncated: bool,
    #[serde(rename = "err-data")]
    err_data: Option<String>,
}

/// # Errors
/// Returns an error for a malformed reply or a pid the agent never issued.
pub(crate) fn exec_pid(response: &str) -> Result<i64, RuntimeError> {
    let started: Started = decode("guest-agent exec", response)?;
    if started.pid <= 0 {
        return Err(RuntimeError::libvirt(
            "guest-agent exec",
            "guest-agent returned an unusable pid",
        ));
    }
    Ok(started.pid)
}

/// An in-band error means the agent can no longer speak for this pid at all,
/// whatever it says: the observation is lost rather than a verdict. That rule
/// is deliberately independent of the agent's wording.
///
/// # Errors
/// Returns an error for a malformed or contradictory reply.
pub(crate) fn exec_outcome(response: &str) -> Result<AgentExecOutcome, RuntimeError> {
    let status: Status = match answer("guest-agent exec status", response)? {
        Answer::Result(status) => status,
        Answer::Refused(reason) => return Ok(AgentExecOutcome::Lost { reason }),
    };
    if !status.exited {
        return Ok(AgentExecOutcome::Running);
    }
    if status.exitcode.is_some() == status.signal.is_some() {
        return Err(RuntimeError::libvirt(
            "guest-agent exec status",
            "guest-agent reported neither one exit code nor one signal",
        ));
    }
    let (stdout, is_decoded_truncated) = captured(status.out_data.as_deref())?;
    // stderr is diagnostic only, so its truncation is not an error signal.
    let (stderr, _) = captured(status.err_data.as_deref())?;
    Ok(AgentExecOutcome::Exited {
        exit_code: status.exitcode,
        signal: status.signal,
        stdout,
        is_truncated: status.is_out_truncated || is_decoded_truncated,
        stderr,
    })
}

fn captured(out_data: Option<&str>) -> Result<(Vec<u8>, bool), RuntimeError> {
    let Some(encoded) = out_data else {
        return Ok((Vec::new(), false));
    };
    let mut decoded = STANDARD.decode(encoded).map_err(|_| {
        RuntimeError::libvirt("guest-agent exec status", "captured output is not base64")
    })?;
    let is_truncated = decoded.len() > MAX_CAPTURED_BYTES;
    decoded.truncate(MAX_CAPTURED_BYTES);
    Ok((decoded, is_truncated))
}
