use std::time::Duration;

use flanforge_core::{RetentionOutcome, RetentionPhase, RetentionResult};
use flanforge_libvirt_wire::{MAX_RETAINED_WARM_GENERATIONS, PublishedWarm, VolumePointer};

use crate::RuntimeError;

use super::{Stopped, WarmRequest};

/// Refuses rather than growing the pool when retirement has not been able to
/// prove enough generations unreferenced. Named so an operator sees the
/// blockage instead of a validation error on a document write.
pub(super) fn ensure_retirement_headroom(published: Option<&PublishedWarm>) -> Result<(), Stopped> {
    let retained = published.map_or(0, |document| document.superseded().len());
    if retained < MAX_RETAINED_WARM_GENERATIONS {
        return Ok(());
    }
    Err(Stopped {
        phase: RetentionPhase::Stage,
        result: RetentionResult::Skipped,
        reason: format!(
            "warm production is halted: {retained} generation(s) are pinned by live or recoverable overlays"
        ),
        is_record_staged: false,
    })
}

pub(super) fn repoint(
    request: &WarmRequest<'_>,
    published: Option<&PublishedWarm>,
    current: VolumePointer,
) -> Result<PublishedWarm, RuntimeError> {
    let logical = request.warm_template.to_string();
    let produced_at = current.produced_at_unix();
    match published {
        Some(document) => document.repointed(
            logical,
            request.allocation.id.into_uuid(),
            produced_at,
            current,
        ),
        None => PublishedWarm::new(
            request.profile.to_string(),
            logical,
            request.allocation.id.into_uuid(),
            produced_at,
            current,
        ),
    }
    .map_err(RuntimeError::manifest)
}

pub(super) fn stop(
    phase: RetentionPhase,
    result: RetentionResult,
    error: impl std::fmt::Display,
) -> Stopped {
    Stopped {
        phase,
        result,
        reason: error.to_string(),
        is_record_staged: false,
    }
}

/// A stop at or after the `Staging` record write, which the rollback has to
/// undo. The write's own failure counts: it may have landed either way.
pub(super) fn stop_staged(
    phase: RetentionPhase,
    result: RetentionResult,
    error: impl std::fmt::Display,
) -> Stopped {
    Stopped {
        is_record_staged: true,
        ..stop(phase, result, error)
    }
}

pub(super) fn outcome(
    result: RetentionResult,
    phase: RetentionPhase,
    reason: impl Into<String>,
    generation: u64,
) -> RetentionOutcome {
    RetentionOutcome::new(result, phase, reason, Some(generation))
}

pub(super) fn remaining(deadline: tokio::time::Instant) -> Result<Duration, Stopped> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| Stopped {
            phase: RetentionPhase::Stage,
            result: RetentionResult::Failed,
            reason: "warm capture exceeded its budget".to_owned(),
            is_record_staged: false,
        })
}

pub(crate) fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
