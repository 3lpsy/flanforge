use flanforge_libvirt_wire::{HelperFailure, HelperFailureCode};
use flanforge_manager::WorkerError;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeError {
    #[error("libvirt configuration is invalid: {message}")]
    Configuration { message: String },
    #[error("libvirt actor is unavailable")]
    ActorUnavailable,
    #[error("libvirt helper failed ({code:?}): {message}")]
    Helper {
        code: HelperFailureCode,
        message: String,
    },
    #[error("libvirt operation exceeded its deadline")]
    Deadline,
    #[error("libvirt {operation} failed: {message}")]
    Libvirt {
        operation: &'static str,
        message: String,
    },
    #[error("libvirt {operation} is temporarily unavailable: {message}")]
    Transient {
        operation: &'static str,
        message: String,
    },
    #[error("libvirt capacity check failed: {message}")]
    Capacity { message: String },
    #[error("libvirt {operation} collided with an existing resource: {message}")]
    Collision {
        operation: &'static str,
        message: String,
    },
    #[error("libvirt ownership check failed: {message}")]
    Ownership { message: String },
    #[error("libvirt manifest is invalid: {message}")]
    Manifest { message: String },
    #[error("libvirt manifest is absent")]
    ManifestNotFound,
    #[error("libvirt seed creation failed: {message}")]
    Seed { message: String },
    #[error("libvirt guest check failed: {message}")]
    Guest { message: String },
    #[error("libvirt standalone operation was cancelled")]
    Cancelled,
    #[error("libvirt cleanup failed after {operation}: {cleanup}")]
    Cleanup { operation: String, cleanup: String },
}

impl RuntimeError {
    pub(crate) fn libvirt(operation: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Libvirt {
            operation,
            message: safe_message(error),
        }
    }

    pub(crate) fn helper(message: impl std::fmt::Display) -> Self {
        Self::Helper {
            code: HelperFailureCode::Internal,
            message: safe_message(message),
        }
    }

    pub(crate) fn from_helper(failure: &HelperFailure) -> Self {
        Self::Helper {
            code: failure.code(),
            message: safe_message(failure.message()),
        }
    }

    pub(crate) fn transient(operation: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Transient {
            operation,
            message: safe_message(error),
        }
    }

    pub(crate) fn capacity(message: impl std::fmt::Display) -> Self {
        Self::Capacity {
            message: safe_message(message),
        }
    }

    pub(crate) fn collision(operation: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Collision {
            operation,
            message: safe_message(message),
        }
    }

    pub(crate) fn helper_failure(&self) -> HelperFailure {
        let code = match self {
            Self::Configuration { .. } => HelperFailureCode::Configuration,
            Self::ActorUnavailable => HelperFailureCode::Unavailable,
            Self::Deadline => HelperFailureCode::Deadline,
            Self::Helper { code, .. } => *code,
            Self::Libvirt { .. } => HelperFailureCode::Libvirt,
            Self::Transient { .. } => HelperFailureCode::Transient,
            Self::Capacity { .. } => HelperFailureCode::Capacity,
            Self::Collision { .. } => HelperFailureCode::Conflict,
            Self::Ownership { .. } => HelperFailureCode::Ownership,
            Self::Manifest { .. } => HelperFailureCode::Manifest,
            Self::ManifestNotFound => HelperFailureCode::NotFound,
            Self::Seed { .. } => HelperFailureCode::Seed,
            Self::Guest { .. } | Self::Cancelled | Self::Cleanup { .. } => {
                HelperFailureCode::Internal
            }
        };
        HelperFailure::new(code, self)
    }

    /// Whether retrying the same call could still succeed. A libvirtd restart
    /// while a guest boots reports this, and the next poll would have worked.
    pub(crate) const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transient { .. }
                | Self::ActorUnavailable
                | Self::Deadline
                | Self::Helper {
                    code: HelperFailureCode::Transient | HelperFailureCode::Unavailable,
                    ..
                }
        )
    }

    pub(crate) fn manifest(message: impl std::fmt::Display) -> Self {
        Self::Manifest {
            message: safe_message(message),
        }
    }

    pub(crate) fn ownership(message: impl std::fmt::Display) -> Self {
        Self::Ownership {
            message: safe_message(message),
        }
    }

    pub(crate) fn seed(error: impl std::fmt::Display) -> Self {
        Self::Seed {
            message: safe_message(error),
        }
    }

    pub(crate) fn guest(error: impl std::fmt::Display) -> Self {
        Self::Guest {
            message: safe_message(error),
        }
    }

    pub(crate) fn cleanup(
        operation: impl std::fmt::Display,
        cleanup: impl std::fmt::Display,
    ) -> Self {
        Self::Cleanup {
            operation: safe_message(operation),
            cleanup: safe_message(cleanup),
        }
    }
}

fn safe_message(value: impl std::fmt::Display) -> String {
    let value = value
        .to_string()
        .chars()
        .map(|character| {
            if character.is_ascii_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    flanforge_core::bounded_text(value, 384)
}

impl From<RuntimeError> for WorkerError {
    fn from(error: RuntimeError) -> Self {
        Self::new(error.to_string())
    }
}
