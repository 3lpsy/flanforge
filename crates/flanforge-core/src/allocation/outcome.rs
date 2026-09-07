use serde::{Deserialize, Serialize};

/// Why a terminal allocation ended, when the reason changes the answer the API
/// owes the caller. Absent means an ordinary allocation failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReason {
    /// The host stayed full for the whole bounded wait: retry later, the build
    /// is not broken.
    Capacity,
}
