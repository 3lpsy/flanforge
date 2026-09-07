use std::fmt;

use async_trait::async_trait;
use flanforge_core::{ProfileName, WarmImageRecord};

use super::StoreError;

#[async_trait]
pub trait WarmImageStore: fmt::Debug + Send + Sync {
    /// # Errors
    ///
    /// Returns an error when the records cannot be listed.
    async fn load_all(&self) -> Result<Vec<WarmImageRecord>, StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record cannot be read.
    async fn load(&self, profile: &ProfileName) -> Result<Option<WarmImageRecord>, StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record is structurally invalid or the write
    /// does not commit.
    async fn save(&self, record: &WarmImageRecord) -> Result<(), StoreError>;

    /// # Errors
    ///
    /// Returns an error when the record cannot be removed.
    async fn remove(&self, profile: &ProfileName) -> Result<(), StoreError>;
}
