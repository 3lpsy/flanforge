use serde::{Deserialize, Serialize};

/// The guest contract that first published the agent exec channel. A base
/// built before it cannot carry the SSH-less channel, whatever else it says.
pub(super) const GUEST_CONTRACT_AGENT_CHANNEL: u8 = 2;

pub(super) const MAX_GUEST_CONTRACT_VERSION: u8 = GUEST_CONTRACT_AGENT_CHANNEL;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Guest {
    pub(super) schema_version: u8,
    #[serde(rename = "guest_contract_version")]
    pub(super) contract_version: u8,
    pub(super) os: GuestOs,
    pub(super) forgejo_runner: GuestTool,
    pub(super) podman: Podman,
    /// Contract 2 and later. Absent decodes a record an older template wrote,
    /// and those records are durable state under `state_dir`.
    #[serde(
        default,
        rename = "guest_agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) agent: Option<GuestAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) job_account: Option<JobAccount>,
    /// The optional privileged automation account; absent when the build
    /// omitted it or the template predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) privileged_account: Option<JobAccount>,
    pub(super) dependency_proxy_configured: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuestOs {
    pub(super) id: String,
    pub(super) version_id: String,
    pub(super) architecture: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuestTool {
    pub(super) version: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Podman {
    pub(super) version: String,
    pub(super) rootless: bool,
    pub(super) docker_api: bool,
}

/// What the base image baked into its QEMU guest agent. `block_rpcs` is
/// recorded so an operator can see what stayed blocked without booting.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuestAgent {
    pub(super) exec_enabled: bool,
    pub(super) block_rpcs: String,
}

/// The unprivileged account the base provides. The agent channel executes as
/// root and drops to it, so the daemon cross-checks the name it configured.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JobAccount {
    pub(super) name: String,
    pub(super) uid: u32,
}
