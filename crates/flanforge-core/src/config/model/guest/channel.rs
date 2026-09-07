use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::super::RuntimeBackendConfig;

/// How the daemon reaches inside a guest.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestChannelKind {
    Ssh,
    /// libvirt only: `guest-exec` on the QEMU guest agent, which is
    /// virtio-serial and host-local to the hypervisor.
    Agent,
}

impl GuestChannelKind {
    /// No `Default` impl on purpose: the default is the backend's, and a
    /// free-standing default would let a `#[serde(default)]` silently pick one.
    ///
    /// A `qemu+tcp` URI resolves to SSH. The administrator already chose to
    /// carry libvirt's API in clear text; a new default must not also put the
    /// job's credentials on that wire without being asked.
    fn for_backend(backend: &RuntimeBackendConfig) -> Self {
        match backend {
            RuntimeBackendConfig::Tart(_) => Self::Ssh,
            RuntimeBackendConfig::Libvirt(libvirt) if libvirt.is_clear_text_transport() => {
                Self::Ssh
            }
            RuntimeBackendConfig::Libvirt(_) => Self::Agent,
        }
    }
}

impl std::fmt::Display for GuestChannelKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ssh => formatter.write_str("ssh"),
            Self::Agent => formatter.write_str("agent"),
        }
    }
}

/// The resolved `[guest]` table. `GuestDocument` below is the schema authority.
#[derive(Clone, Debug, Serialize)]
pub struct GuestConfig {
    pub channel: GuestChannelKind,
    pub runner_user: String,
    /// The privileged automation account the base optionally bakes; the daemon
    /// drives it for root-needing guest work such as clone generalization.
    pub privileged_user: String,
    pub forgejo_runner_path: PathBuf,
    /// Absent means the guest is driven without SSH at all: no key reaches the
    /// seed and the daemon needs no route to the guest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh: Option<GuestSshConfig>,
}

/// Every field stays required or defaulted inside the table, so `[guest.ssh]`
/// cannot be half-configured the way five independent options could be.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestSshConfig {
    pub identity_file: PathBuf,
    /// The privileged account's key; absent falls back to `identity_file`.
    #[serde(default)]
    pub privileged_identity_file: Option<PathBuf>,
    #[serde(default)]
    pub known_hosts_file: Option<PathBuf>,
    #[serde(default)]
    pub host_key_alias: Option<String>,
    #[serde(default = "default_connect_timeout_seconds")]
    pub connect_timeout_seconds: u64,
    #[serde(default = "default_verify_host_key")]
    pub verify_host_key: bool,
}

/// The `[guest]` table as written, before the per-backend channel default is
/// resolved against the runtime backend the same document declares.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GuestDocument {
    #[serde(default)]
    channel: Option<GuestChannelKind>,
    runner_user: String,
    #[serde(default = "default_privileged_user")]
    privileged_user: String,
    forgejo_runner_path: PathBuf,
    #[serde(default)]
    ssh: Option<GuestSshConfig>,
}

impl GuestDocument {
    pub(crate) fn resolve(self, backend: &RuntimeBackendConfig) -> GuestConfig {
        GuestConfig {
            channel: self
                .channel
                .unwrap_or_else(|| GuestChannelKind::for_backend(backend)),
            runner_user: self.runner_user,
            privileged_user: self.privileged_user,
            forgejo_runner_path: self.forgejo_runner_path,
            ssh: self.ssh,
        }
    }
}

fn default_privileged_user() -> String {
    "prunner".to_owned()
}

const fn default_connect_timeout_seconds() -> u64 {
    5
}

const fn default_verify_host_key() -> bool {
    true
}
