use std::path::Path;

use uuid::Uuid;

use crate::{
    WireError,
    libvirt_helper::{
        limits::{
            MAX_AGENT_ARGUMENT_BYTES, MAX_AGENT_ARGUMENTS, MAX_AGENT_INPUT_BYTES,
            MAX_AGENT_OUTPUT_BYTES, MAX_AGENT_REASON_BYTES, MAX_GUEST_SCRIPT_BYTES,
            MAX_HELPER_PATH_BYTES, MAX_LOGIN_PRELUDE_BYTES,
        },
        model::{AgentExecOutcome, AgentExecRequest},
    },
    validation::{is_normal_absolute, is_safe_account_name},
};

use super::shared::invalid;

const RUNUSER: &str = "/usr/sbin/runuser";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const GUEST_READY: &str = "/usr/local/libexec/flanforge-guest-ready";

/// One day. The unit's own deadline is a backstop for a daemon that died, not
/// a job budget, so it only has to be finite.
const MAX_RUNTIME_MAX_SEC: u64 = 86_400;

/// Refused as a backstop, not as the guard: the agent already executes as root
/// and the whole point of the wrapper is dropping out of it. The real guard is
/// the daemon's cross-check against the base image's recorded job account uid,
/// which this boundary cannot see.
const PRIVILEGED_ACCOUNT: &str = "root";
const MAX_READY_WAIT_SECONDS: u64 = 120;

/// The helper runs with libvirt's authority, so an allow-list on `path` alone
/// is not containment: `runuser -l root -c …`, `systemd-run <anything>`, and
/// `systemctl start …` all pass one. Exactly four argv shapes are accepted, and
/// each is matched element by element.
pub(in crate::libvirt_helper) fn ensure_exec_valid(
    request: &AgentExecRequest,
    timeout_seconds: u8,
) -> Result<(), WireError> {
    ensure_timeout(timeout_seconds)?;
    if !is_normal_absolute(Path::new(&request.path), MAX_HELPER_PATH_BYTES)
        || request.arguments.len() > MAX_AGENT_ARGUMENTS
        || request
            .arguments
            .iter()
            .map(String::len)
            .try_fold(0_usize, usize::checked_add)
            .is_none_or(|total| total > MAX_AGENT_ARGUMENT_BYTES)
        || request
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
    {
        return invalid("agent exec argv");
    }
    if request
        .input
        .as_ref()
        .is_some_and(|input| input.is_empty() || input.len() > MAX_AGENT_INPUT_BYTES)
    {
        return invalid("agent exec input");
    }
    // Captured output from a secret-carrying command is never produced, so it
    // can never reach a log.
    if request.input.is_some() && request.is_output_captured {
        return invalid("agent exec capture");
    }
    ensure_shape(request)
}

fn ensure_shape(request: &AgentExecRequest) -> Result<(), WireError> {
    let arguments = request
        .arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    match (request.path.as_str(), arguments.as_slice()) {
        (RUNUSER, ["-l", account, "-c", script]) => ensure_login_shell(account, script),
        (
            SYSTEMD_RUN,
            [
                unit,
                "--pipe",
                "--wait",
                runtime_max,
                "--property=TimeoutStopSec=10",
                RUNUSER,
                "-l",
                account,
                "-c",
                script,
            ],
        ) => {
            ensure_job_unit(unit.strip_prefix("--unit=").unwrap_or_default())?;
            ensure_runtime_max(runtime_max)?;
            ensure_login_shell(account, script)
        }
        (SYSTEMCTL, ["stop", unit]) => ensure_capture_free(request).and_then(|()| {
            ensure_input_free(request)?;
            ensure_job_unit(unit)
        }),
        (GUEST_READY, ["--wait-seconds", seconds]) => {
            ensure_input_free(request)?;
            ensure_bounded_seconds(seconds, MAX_READY_WAIT_SECONDS, "agent exec wait seconds")
        }
        _ => invalid("agent exec shape"),
    }
}

/// The account the agent's root process drops to. `is_safe_name` alone would
/// admit a leading `-`, which `runuser` would read as an option.
fn ensure_login_shell(account: &str, script: &str) -> Result<(), WireError> {
    if !is_safe_account_name(account)
        || account == PRIVILEGED_ACCOUNT
        || script.is_empty()
        || script.len() > MAX_GUEST_SCRIPT_BYTES + MAX_LOGIN_PRELUDE_BYTES
        || script.contains('\0')
    {
        return invalid("agent exec login shell");
    }
    Ok(())
}

/// Rebuilt from the parsed UUID and compared, so no other unit name can be
/// spelled into a stop or a start.
fn ensure_job_unit(unit: &str) -> Result<(), WireError> {
    let parsed = unit
        .strip_prefix("flanforge-job-")
        .and_then(|value| value.strip_suffix(".service"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil());
    match parsed {
        Some(id) if format!("flanforge-job-{id}.service") == unit => Ok(()),
        _ => invalid("agent exec unit"),
    }
}

fn ensure_runtime_max(property: &str) -> Result<(), WireError> {
    let Some(seconds) = property.strip_prefix("--property=RuntimeMaxSec=") else {
        return invalid("agent exec runtime bound");
    };
    ensure_bounded_seconds(seconds, MAX_RUNTIME_MAX_SEC, "agent exec runtime bound")
}

fn ensure_bounded_seconds(value: &str, maximum: u64, field: &'static str) -> Result<(), WireError> {
    let parsed = value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse::<u64>().ok())
        .flatten();
    match parsed {
        Some(seconds) if (1..=maximum).contains(&seconds) => Ok(()),
        _ => Err(WireError::invalid(super::shared::CONTRACT, field)),
    }
}

fn ensure_capture_free(request: &AgentExecRequest) -> Result<(), WireError> {
    if request.is_output_captured {
        return invalid("agent exec capture");
    }
    Ok(())
}

fn ensure_input_free(request: &AgentExecRequest) -> Result<(), WireError> {
    if request.input.is_some() {
        return invalid("agent exec input");
    }
    Ok(())
}

pub(in crate::libvirt_helper) fn ensure_probe_valid(timeout_seconds: u8) -> Result<(), WireError> {
    ensure_timeout(timeout_seconds)
}

pub(in crate::libvirt_helper) fn ensure_status_valid(
    pid: i64,
    timeout_seconds: u8,
) -> Result<(), WireError> {
    ensure_timeout(timeout_seconds)?;
    if pid <= 0 {
        return invalid("agent exec pid");
    }
    Ok(())
}

/// The same bound `Address` uses: the helper clamps its own agent call into
/// this range, so a wider request could only be a mistake.
fn ensure_timeout(timeout_seconds: u8) -> Result<(), WireError> {
    if !(1..=5).contains(&timeout_seconds) {
        return invalid("agent exec timeout");
    }
    Ok(())
}

pub(in crate::libvirt_helper) fn ensure_outcome_valid(
    outcome: &AgentExecOutcome,
) -> Result<(), WireError> {
    match outcome {
        AgentExecOutcome::Running => Ok(()),
        AgentExecOutcome::Lost { reason } => {
            if reason.is_empty()
                || reason.len() > MAX_AGENT_REASON_BYTES
                || reason
                    .bytes()
                    .any(|byte| !byte.is_ascii_graphic() && byte != b' ')
            {
                return invalid("agent exec lost reason");
            }
            Ok(())
        }
        AgentExecOutcome::Exited {
            exit_code,
            signal,
            stdout,
            is_truncated: _,
            stderr,
        } => {
            if stdout.len() > MAX_AGENT_OUTPUT_BYTES
                || stderr.len() > MAX_AGENT_OUTPUT_BYTES
                || exit_code.is_some() == signal.is_some()
            {
                return invalid("agent exec outcome");
            }
            Ok(())
        }
    }
}
