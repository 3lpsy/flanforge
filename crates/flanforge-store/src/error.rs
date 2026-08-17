use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("state serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("invalid allocation state in {path}: {source}")]
    InvalidState {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("allocation state in {path} is structurally invalid")]
    InvalidAllocation { path: PathBuf },
    #[error("warm image record in {path} is structurally invalid")]
    InvalidWarmImage { path: PathBuf },
    #[error("cannot acquire state lock {path}: {source}")]
    Lock {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("state directory sync task failed")]
    Sync,
}
