use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::{AllocationId, IdentifierError, ProfileName, VmName, image::BaseFingerprint};

/// Reserved suffix of the candidate image mid-promotion.
pub const STAGING_SUFFIX: &str = ".staging";
/// Reserved suffix of the single retained rollback generation.
pub const PREVIOUS_SUFFIX: &str = ".previous";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmImageState {
    Staging,
    Promoted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct WarmGeneration {
    #[validate(range(min = 1))]
    pub generation: u64,
    #[validate(nested)]
    pub base_fingerprint: BaseFingerprint,
    pub produced_by: AllocationId,
    pub produced_at_unix: u64,
}

/// Provenance of one profile's warm image; a consumer reads it once per
/// allocation instead of scanning allocation records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct WarmImageRecord {
    #[validate(nested)]
    pub profile: ProfileName,
    #[validate(nested)]
    pub warm_template: VmName,
    #[validate(range(min = 1))]
    pub generation: u64,
    #[validate(nested)]
    pub base_fingerprint: BaseFingerprint,
    pub produced_by: AllocationId,
    pub produced_at_unix: u64,
    pub state: WarmImageState,
    #[validate(nested)]
    pub previous: Option<WarmGeneration>,
}

impl WarmImageRecord {
    /// Name of the candidate image mid-promotion.
    ///
    /// # Errors
    ///
    /// Returns an error when the derived name exceeds the VM-name length.
    pub fn staging_name(&self) -> Result<VmName, IdentifierError> {
        self.warm_template.with_suffix(STAGING_SUFFIX)
    }

    /// Name of the retained rollback generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the derived name exceeds the VM-name length.
    pub fn previous_name(&self) -> Result<VmName, IdentifierError> {
        self.warm_template.with_suffix(PREVIOUS_SUFFIX)
    }
}
