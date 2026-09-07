/// Why a worker stopped. `Capacity` is the host being full for a whole bounded
/// wait, which the API owes the caller as busy rather than as a failed build.
#[derive(Clone, Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Capacity(String),
}

impl WorkerError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(flanforge_core::bounded_text(message, 512))
    }

    /// No slot came free before the deadline: retry later, nothing is broken.
    #[must_use]
    pub fn capacity(message: impl Into<String>) -> Self {
        Self::Capacity(flanforge_core::bounded_text(message, 512))
    }

    #[must_use]
    pub const fn is_capacity(&self) -> bool {
        matches!(self, Self::Capacity(_))
    }
}
