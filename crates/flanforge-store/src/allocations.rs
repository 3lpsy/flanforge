use std::fmt;

use async_trait::async_trait;
use flanforge_core::Allocation;

use super::StoreError;

#[async_trait]
pub trait AllocationStore: fmt::Debug + Send + Sync {
    /// Loads the newest `limit` records, plus every older record that is not
    /// terminal. The bound applies to finished history only: unfinished work
    /// can still own a VM and a runner registration, so it is never dropped.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable records cannot be listed or read.
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError>;

    /// Persists one allocation record.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is structurally invalid or the write
    /// does not commit.
    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError>;
}
