use serde::{Deserialize, de::IgnoredAny};

use crate::RuntimeError;

use super::envelope::decode;

/// Fedora ships around sixty commands; the cap bounds a hostile reply without
/// coming near a real one.
const MAX_SUPPORTED_COMMANDS: usize = 256;

#[derive(Deserialize)]
struct Info {
    #[serde(rename = "supported_commands", default)]
    commands: Vec<SupportedCommand>,
}

#[derive(Deserialize)]
struct SupportedCommand {
    name: String,
    enabled: bool,
}

/// Proves the agent is alive and in sync. No guest process is started.
///
/// # Errors
/// Returns an error when the agent does not answer with an empty result.
pub(crate) fn ensure_ping_answered(response: &str) -> Result<(), RuntimeError> {
    decode::<IgnoredAny>("guest-agent ping", response).map(|_| ())
}

/// Whether the agent will run `name`. A blocked RPC is a configuration fact
/// about the image, not a transient condition, so asking once beats retrying
/// an in-band error until the boot deadline.
///
/// # Errors
/// Returns an error for a malformed or oversized reply.
pub(crate) fn exec_capability(response: &str, name: &str) -> Result<bool, RuntimeError> {
    let info: Info = decode("guest-agent info", response)?;
    if info.commands.len() > MAX_SUPPORTED_COMMANDS {
        return Err(RuntimeError::libvirt(
            "guest-agent info",
            "guest-agent listed too many commands",
        ));
    }
    Ok(info
        .commands
        .iter()
        .any(|command| command.name == name && command.enabled))
}
