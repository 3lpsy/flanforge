use flanforge_core::ProfileName;

#[derive(Debug, thiserror::Error)]
pub enum OperatorError {
    #[error("standalone libvirt configuration is invalid: {message}")]
    Configuration { message: String },
    #[error("profile {0} is not configured")]
    ProfileNotFound(ProfileName),
    #[error("standalone libvirt operation was cancelled")]
    Cancelled,
    #[error(transparent)]
    State(#[from] flanforge_store::StoreError),
    #[error(transparent)]
    Runtime(#[from] flanforge_runtime_libvirt::RuntimeError),
}
