use serde::{Deserialize, de::DeserializeOwned};

use crate::RuntimeError;

/// Every guest-agent reply is read into memory whole, so the bound is the only
/// thing standing between a guest and the helper's address space.
pub(super) const MAX_QGA_BYTES: usize = 64 * 1_024;
const MAX_ERROR_CLASS_BYTES: usize = 64;
const MAX_ERROR_DESC_BYTES: usize = 256;

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(rename = "return")]
    result: Option<T>,
    error: Option<AgentError>,
}

#[derive(Deserialize)]
struct AgentError {
    class: String,
    desc: String,
}

/// One decoded reply. The agent answered either way; `Refused` is its own
/// in-band error, which is a fact about the guest rather than a transport
/// failure and which some callers treat as an outcome.
pub(super) enum Answer<T> {
    Result(T),
    Refused(String),
}

/// # Errors
/// Returns an error for an oversized, malformed, or resultless reply.
pub(super) fn answer<T: DeserializeOwned>(
    operation: &'static str,
    response: &str,
) -> Result<Answer<T>, RuntimeError> {
    if response.is_empty() || response.len() > MAX_QGA_BYTES {
        return Err(RuntimeError::libvirt(
            operation,
            "guest-agent response size is invalid",
        ));
    }
    let envelope: Envelope<T> =
        serde_json::from_str(response).map_err(|error| RuntimeError::libvirt(operation, error))?;
    if let Some(error) = envelope.error {
        if error.class.len() > MAX_ERROR_CLASS_BYTES || error.desc.len() > MAX_ERROR_DESC_BYTES {
            return Err(RuntimeError::libvirt(
                operation,
                "guest-agent error is oversized",
            ));
        }
        return Ok(Answer::Refused(format!("{}: {}", error.class, error.desc)));
    }
    envelope
        .result
        .map(Answer::Result)
        .ok_or_else(|| RuntimeError::libvirt(operation, "guest-agent returned no result"))
}

/// The same decode for callers that have no use for an in-band error beyond
/// failing on it.
///
/// # Errors
/// Returns an error for an oversized, malformed, or refused reply.
pub(super) fn decode<T: DeserializeOwned>(
    operation: &'static str,
    response: &str,
) -> Result<T, RuntimeError> {
    match answer(operation, response)? {
        Answer::Result(value) => Ok(value),
        Answer::Refused(message) => Err(RuntimeError::libvirt(operation, message)),
    }
}
