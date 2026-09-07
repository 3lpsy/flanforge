use flanforge_core::{
    Allocation, BaseFingerprint, IdentifierError, PREVIOUS_SUFFIX, Profile, RetentionOutcome,
    RetentionPhase, RetentionResult, STAGING_SUFFIX, VmName, WarmGeneration,
};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::{AllocationReporter, WorkerError};

pub(crate) struct RetentionRequest<'a> {
    pub(crate) allocation: &'a Allocation,
    pub(crate) profile: &'a Profile,
    pub(crate) warm_template: VmName,
    pub(crate) base_fingerprint: BaseFingerprint,
    pub(crate) generation: u64,
    pub(crate) previous: Option<WarmGeneration>,
}

/// The three names one profile's promotion touches.
pub(super) struct Names {
    pub(super) warm: VmName,
    pub(super) staging: VmName,
    pub(super) previous: VmName,
}

/// Where the sequence stopped, and what the rollback still has to work with.
pub(super) struct Stopped<'a> {
    pub(super) phase: RetentionPhase,
    pub(super) error: &'a WorkerError,
    pub(super) deadline: tokio::time::Instant,
}

/// The live guest and the channels the sequence reports through.
pub(super) struct Session<'a> {
    pub(super) vm: &'a mut Child,
    pub(super) ip: &'a str,
    pub(super) reporter: &'a AllocationReporter,
    pub(super) cancellation: &'a CancellationToken,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) fn derive(warm: &VmName) -> Result<Names, IdentifierError> {
    Ok(Names {
        warm: warm.clone(),
        staging: warm.with_suffix(STAGING_SUFFIX)?,
        previous: warm.with_suffix(PREVIOUS_SUFFIX)?,
    })
}

pub(super) fn outcome(
    result: RetentionResult,
    phase: RetentionPhase,
    reason: impl Into<String>,
    generation: u64,
) -> RetentionOutcome {
    RetentionOutcome::new(result, phase, reason, Some(generation))
}

pub(super) fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
