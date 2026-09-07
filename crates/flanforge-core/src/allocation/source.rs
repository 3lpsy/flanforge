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
///
/// Deserialization is deliberately tolerant: an older reader must be able to
/// parse an allocation listing written by a newer daemon rather than failing
/// the whole document on one unknown reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    /// The runtime backend does not declare the warm-image capability.
    Unsupported,
    /// No backend produces this now that a guest is sized at `max(storage_mb,
    /// base virtual size)`; retained only to decode pre-clamp records.
    StorageTooSmall,
    /// A reason this build does not know; carried so one unknown value cannot
    /// fail an entire allocation listing.
    Unknown,
}

impl FallbackReason {
    const NAMES: [(&'static str, Self); 12] = [
        ("not_declared", Self::NotDeclared),
        ("no_record", Self::NoRecord),
        ("repointed", Self::Repointed),
        ("absent", Self::Absent),
        ("not_stopped", Self::NotStopped),
        ("stale_base", Self::StaleBase),
        ("unclaimed", Self::Unclaimed),
        ("quarantined", Self::Quarantined),
        ("not_promoted", Self::NotPromoted),
        ("fingerprint_unavailable", Self::FingerprintUnavailable),
        ("unsupported", Self::Unsupported),
        ("storage_too_small", Self::StorageTooSmall),
    ];
}

impl<'de> Deserialize<'de> for FallbackReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::NAMES
            .iter()
            .find(|(name, _)| *name == value)
            .map_or(Self::Unknown, |(_, reason)| *reason))
    }
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
