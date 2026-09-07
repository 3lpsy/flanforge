use flanforge_manager::ManagerError;
use flanforge_orm::UserError;
use thiserror::Error;

/// Why a web UI action was refused. The transport layer maps each variant to
/// one status and one safe message; internals are logged here, never shown.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WebuiFault {
    #[error("authentication required")]
    Unauthenticated,
    #[error("request is not authorized")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
    /// A refusal whose safe, specific reason is worth showing: which key was
    /// not editable, which value did not fit, which rule failed.
    #[error("{0}")]
    Rejected(String),
    #[error("internal service error")]
    Internal,
}

impl From<UserError> for WebuiFault {
    fn from(error: UserError) -> Self {
        match error {
            UserError::Db(message) => {
                tracing::error!(%message, "webui store failed");
                Self::Internal
            }
            UserError::UsernameTaken => Self::Conflict("username is already taken"),
            UserError::NotFound => Self::NotFound,
            UserError::ProviderManaged => {
                Self::Conflict("account is managed by the identity provider")
            }
        }
    }
}

impl From<ManagerError> for WebuiFault {
    fn from(error: ManagerError) -> Self {
        match &error {
            ManagerError::NotFound(_) | ManagerError::UnknownHotGuest(_) => Self::NotFound,
            ManagerError::ShuttingDown => Self::Conflict("service is shutting down"),
            _ => {
                tracing::error!(%error, "webui manager action failed");
                Self::Internal
            }
        }
    }
}

impl From<sea_orm::DbErr> for WebuiFault {
    fn from(error: sea_orm::DbErr) -> Self {
        tracing::error!(%error, "webui history query failed");
        Self::Internal
    }
}
