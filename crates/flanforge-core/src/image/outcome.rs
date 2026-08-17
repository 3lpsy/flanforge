use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::bounded_text;

const MAX_REASON_LEN: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionResult {
    Promoted,
    RolledBack,
    Skipped,
    Rejected,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPhase {
    Strip,
    Stop,
    Stage,
    Verify,
    Retire,
    Promote,
}

/// Where retention stopped and what it produced.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct RetentionOutcome {
    pub outcome: RetentionResult,
    pub phase: RetentionPhase,
    #[validate(custom(function = "validate_reason"))]
    pub reason: String,
    pub generation: Option<u64>,
}

impl RetentionOutcome {
    /// Bounds the reason at construction; validation must never be the first
    /// place a value is rejected.
    #[must_use]
    pub fn new(
        outcome: RetentionResult,
        phase: RetentionPhase,
        reason: impl Into<String>,
        generation: Option<u64>,
    ) -> Self {
        Self {
            outcome,
            phase,
            reason: bounded_text(reason, MAX_REASON_LEN),
            generation,
        }
    }
}

fn validate_reason(value: &str) -> Result<(), ValidationError> {
    if value.len() <= MAX_REASON_LEN && !value.contains('\0') {
        Ok(())
    } else {
        Err(ValidationError::new("retention_reason"))
    }
}
