use serde::{Deserialize, Serialize};

/// The trust lane a hot machine is pinned to, derived from the signed
/// `ref_protected` claim. Two machines in different lanes never meet.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotLane {
    Protected,
    Unprotected,
}

impl HotLane {
    #[must_use]
    pub const fn from_ref_protected(is_ref_protected: bool) -> Self {
        if is_ref_protected {
            Self::Protected
        } else {
            Self::Unprotected
        }
    }
}

/// Which lanes a profile admits hot reuse for. `None` is a live kill switch
/// that leaves the rest of the table in place.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotLanePolicy {
    None,
    #[default]
    Protected,
    Any,
}

impl HotLanePolicy {
    #[must_use]
    pub const fn is_lane_admitted(self, lane: HotLane) -> bool {
        match self {
            Self::None => false,
            Self::Protected => matches!(lane, HotLane::Protected),
            Self::Any => true,
        }
    }
}
