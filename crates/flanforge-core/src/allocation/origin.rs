use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use crate::VmName;

/// Where the guest this allocation ran on came from. `CloneSource` records
/// what was cloned and a hot claim clones nothing, so provenance needs its own
/// field: an operator reading `flanforged allocation` must be able to tell in
/// one line that a job ran on a machine with history.
///
/// The hot machine's own `CloneSource` is copied onto the allocation's `source`
/// at claim time, so this carries only what the clone path cannot say.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AllocationOrigin {
    #[default]
    Cloned,
    HotReuse {
        vm_name: VmName,
        /// Jobs the machine had already served when this allocation claimed it.
        jobs_served: u32,
        booted_at_unix: u64,
    },
}

impl AllocationOrigin {
    /// True when a previous job's machine served this allocation, which is the
    /// one fact the weaker boundary rests on.
    #[must_use]
    pub const fn is_hot_reuse(&self) -> bool {
        matches!(self, Self::HotReuse { .. })
    }

    /// The machine a hot claim bound, so recovery can release it instead of
    /// cleaning it.
    #[must_use]
    pub const fn hot_vm_name(&self) -> Option<&VmName> {
        match self {
            Self::Cloned => None,
            Self::HotReuse { vm_name, .. } => Some(vm_name),
        }
    }
}

/// Hand-written because `derive(Validate)` covers structs only. The machine
/// name is the one field that can be forged by a hand-edited record.
impl Validate for AllocationOrigin {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::Cloned => Ok(()),
            Self::HotReuse { vm_name, .. } => vm_name.validate(),
        }
    }
}
