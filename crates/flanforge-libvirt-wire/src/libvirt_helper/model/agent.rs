use serde::{Deserialize, Serialize};

/// One `guest-exec` invocation, exactly as the daemon asks for it. The helper
/// is root-equivalent, so the shape of `arguments` is validated and not merely
/// its first element.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentExecRequest {
    pub path: String,
    pub arguments: Vec<String>,
    /// The process's whole stdin, and the only place a guest secret crosses
    /// this boundary. Never logged, never echoed in a failure message.
    pub input: Option<Vec<u8>>,
    pub is_output_captured: bool,
}

/// `HelperRequest` derives `Debug`, so without this the registration token
/// would be one `tracing::debug!` away from a log file.
impl std::fmt::Debug for AgentExecRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentExecRequest")
            .field("path", &self.path)
            .field("arguments", &self.arguments)
            .field("input_bytes", &self.input.as_ref().map_or(0, Vec::len))
            .field("is_output_captured", &self.is_output_captured)
            .finish()
    }
}

/// What one `guest-exec-status` reply said. The agent frees its exec record on
/// the first reply reporting `exited`, so a terminal outcome is read once.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentExecOutcome {
    Running,
    /// The agent no longer knows this pid, because it or the guest restarted.
    /// Never a verdict on the process, which may still be running.
    Lost {
        reason: String,
    },
    Exited {
        exit_code: Option<i32>,
        signal: Option<i32>,
        /// Present only when the command asked for it; bounded by the helper
        /// and re-bounded by the daemon that reads it.
        stdout: Vec<u8>,
        is_truncated: bool,
        /// Diagnostic only, so a failure can name its cause; never parsed.
        #[serde(default)]
        stderr: Vec<u8>,
    },
}
