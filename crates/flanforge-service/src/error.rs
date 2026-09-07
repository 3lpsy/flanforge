#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("cannot load service configuration")]
    Configuration {
        #[source]
        source: anyhow::Error,
    },
    #[error("service {operation} failed")]
    Operation {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

impl ServiceError {
    #[doc(hidden)]
    #[must_use]
    pub fn configuration(source: anyhow::Error) -> Self {
        Self::Configuration { source }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn operation(operation: &'static str, source: anyhow::Error) -> Self {
        Self::Operation { operation, source }
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;
