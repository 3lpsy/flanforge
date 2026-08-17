use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::{VmName, image::BaseFingerprint};

/// What an allocation boots from and whether it may produce an image.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationMode {
    #[default]
    Cold,
    Warm,
    Regenerate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneKind {
    Template,
    Warm,
}

/// Why a warm request ended up on the profile template.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    NotDeclared,
    NoRecord,
    Repointed,
    Absent,
    NotStopped,
    StaleBase,
    Unclaimed,
    Quarantined,
    /// The record describes a promotion that has not finished, so the image
    /// under the name is not the generation the record claims.
    NotPromoted,
    FingerprintUnavailable,
}

/// The VM this allocation actually cloned, recorded at clone time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct CloneSource {
    #[validate(nested)]
    pub name: VmName,
    pub kind: CloneKind,
    #[validate(nested)]
    pub base_fingerprint: Option<BaseFingerprint>,
    pub fallback_reason: Option<FallbackReason>,
}
