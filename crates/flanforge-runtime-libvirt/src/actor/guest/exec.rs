use std::time::Instant;

use base64::{Engine, engine::general_purpose::STANDARD};
use flanforge_libvirt_wire::{AgentExecOutcome, AgentExecRequest, OwnershipManifest};
use serde::Serialize;
use serde_json::json;
use virt::connect::Connect;

use crate::{
    RuntimeError,
    domain::{ensure_ping_answered, exec_capability, exec_outcome, exec_pid},
};

use super::agent_command;

/// `virt 0.4.3` builds the command's `CString` with an unchecked unwrap, so an
/// interior NUL would abort the helper. Every command string is therefore built
/// with `serde_json`, which escapes NUL and can only produce a NUL-free string.
fn command(value: &impl Serialize) -> Result<String, RuntimeError> {
    serde_json::to_string(value)
        .map_err(|error| RuntimeError::libvirt("guest-agent command", error))
}

/// Proves the agent is alive and in sync without starting a guest process.
pub(in crate::actor) fn ping(
    connection: &Connect,
    manifest: &OwnershipManifest,
    deadline: Instant,
) -> Result<(), RuntimeError> {
    let request = command(&json!({"execute": "guest-ping"}))?;
    ensure_ping_answered(&agent_command(connection, manifest, &request, deadline)?)
}

/// Whether this guest will run `guest-exec`. Fedora's packaging blocks it by
/// default, and a blocked RPC never becomes unblocked by waiting.
pub(in crate::actor) fn is_exec_enabled(
    connection: &Connect,
    manifest: &OwnershipManifest,
    deadline: Instant,
) -> Result<bool, RuntimeError> {
    let request = command(&json!({"execute": "guest-info"}))?;
    let response = agent_command(connection, manifest, &request, deadline)?;
    Ok(exec_capability(&response, "guest-exec")?
        && exec_capability(&response, "guest-exec-status")?)
}

/// Starts one command and returns the guest pid that only `guest-exec-status`
/// on this same domain can interpret.
pub(in crate::actor) fn exec(
    connection: &Connect,
    manifest: &OwnershipManifest,
    request: &AgentExecRequest,
    deadline: Instant,
) -> Result<i64, RuntimeError> {
    let mut arguments = json!({"path": request.path, "arg": request.arguments});
    let object = arguments
        .as_object_mut()
        .ok_or_else(|| RuntimeError::libvirt("guest-agent exec", "command is not an object"))?;
    if let Some(input) = request.input.as_ref() {
        object.insert("input-data".to_owned(), STANDARD.encode(input).into());
    }
    if request.is_output_captured {
        object.insert("capture-output".to_owned(), true.into());
    }
    let request = command(&json!({"execute": "guest-exec", "arguments": arguments}))?;
    exec_pid(&agent_command(connection, manifest, &request, deadline)?)
}

pub(in crate::actor) fn exec_status(
    connection: &Connect,
    manifest: &OwnershipManifest,
    pid: i64,
    deadline: Instant,
) -> Result<AgentExecOutcome, RuntimeError> {
    let request = command(&json!({
        "execute": "guest-exec-status",
        "arguments": {"pid": pid},
    }))?;
    exec_outcome(&agent_command(connection, manifest, &request, deadline)?)
}
