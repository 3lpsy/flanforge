mod fingerprint;
mod outcome;
mod record;

pub use fingerprint::{BaseFingerprint, FileStat, is_fingerprint_match};
pub use outcome::{RetentionOutcome, RetentionPhase, RetentionResult};
pub use record::{
    PREVIOUS_SUFFIX, STAGING_SUFFIX, WarmGeneration, WarmImageRecord, WarmImageState,
};

#[cfg(test)]
mod tests;
