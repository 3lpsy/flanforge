use std::fmt;

use async_trait::async_trait;
use flanforge_core::{HotGuest, VmName};

use super::StoreError;

#[async_trait]
pub trait HotGuestStore: fmt::Debug + Send + Sync {
    /// # Errors
    ///
    /// Returns an error when the records cannot be listed.
    async fn load_all(&self) -> Result<Vec<HotGuest>, StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record cannot be read.
    async fn load(&self, vm_name: &VmName) -> Result<Option<HotGuest>, StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record is structurally invalid or the write
    /// does not commit.
    async fn save(&self, guest: &HotGuest) -> Result<(), StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record cannot be removed.
    async fn remove(&self, vm_name: &VmName) -> Result<(), StoreError>;
}
