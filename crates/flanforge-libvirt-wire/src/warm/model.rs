use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{VolumePointer, WireError};

use super::MAX_PUBLISHED_WARM_BYTES;

const CONTRACT: &str = "published libvirt warm image";

/// One profile's warm image: the generation allocations boot from, plus every
/// superseded generation still awaiting a provable retirement.
///
/// Physical volume names are never reused, so repointing this document is the
/// whole of promotion and a live consumer's frozen backing path can only ever
/// resolve to the exact bytes it booted from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedWarm {
    schema_version: u8,
    profile: String,
    logical_name: String,
    generation: u64,
    produced_by: Uuid,
    produced_at_unix: u64,
    current: VolumePointer,
    #[serde(default)]
    superseded: Vec<VolumePointer>,
}

impl PublishedWarm {
    /// Builds the first document for a profile.
    ///
    /// # Errors
    /// Returns an error when any identity or pointer field is invalid.
    pub fn new(
        profile: String,
        logical_name: String,
        produced_by: Uuid,
        produced_at_unix: u64,
        current: VolumePointer,
    ) -> Result<Self, WireError> {
        let document = Self {
            schema_version: 1,
            profile,
            logical_name,
            generation: current.generation(),
            produced_by,
            produced_at_unix,
            current,
            superseded: Vec::new(),
        };
        document.ensure_valid()?;
        Ok(document)
    }

    /// Repoints the profile at a new generation, retaining the outgoing one.
    ///
    /// # Errors
    /// Returns an error when the result is not structurally valid, which the
    /// promotion pre-flight cap exists to make unreachable.
    pub fn repointed(
        &self,
        logical_name: String,
        produced_by: Uuid,
        produced_at_unix: u64,
        current: VolumePointer,
    ) -> Result<Self, WireError> {
        let mut superseded = vec![self.current.clone()];
        superseded.extend(self.superseded.iter().cloned());
        let document = Self {
            schema_version: 1,
            profile: self.profile.clone(),
            logical_name,
            generation: current.generation(),
            produced_by,
            produced_at_unix,
            current,
            superseded,
        };
        document.ensure_valid()?;
        Ok(document)
    }

    /// Promotes one superseded generation back to current, for a rollback the
    /// record already reverted.
    ///
    /// The outgoing current becomes superseded rather than being dropped: a
    /// generation no document names could never again be proven ours, and
    /// would strand a full-size volume no sweep could collect.
    ///
    /// A rolled-back number is re-issued, so two entries may carry it; the
    /// most recently superseded one wins, and both this and `repointed`
    /// prepend, so that is the lowest matching index.
    ///
    /// # Errors
    /// Returns an error when no superseded entry carries that generation.
    pub fn restored(&self, generation: u64) -> Result<Self, WireError> {
        let index = self
            .superseded
            .iter()
            .position(|pointer| pointer.generation() == generation)
            .ok_or_else(|| WireError::invalid(CONTRACT, "restored generation"))?;
        let mut superseded = self.superseded.clone();
        let current = superseded.remove(index);
        superseded.insert(0, self.current.clone());
        let document = Self {
            schema_version: 1,
            profile: self.profile.clone(),
            logical_name: self.logical_name.clone(),
            generation: current.generation(),
            produced_by: self.produced_by,
            produced_at_unix: current.produced_at_unix(),
            current,
            superseded,
        };
        document.ensure_valid()?;
        Ok(document)
    }

    /// Drops one retired superseded entry by volume key.
    ///
    /// # Errors
    /// Returns an error when the remaining document is not valid.
    pub fn without_superseded(&self, volume_key: &str) -> Result<Self, WireError> {
        let mut document = self.clone();
        document
            .superseded
            .retain(|pointer| pointer.volume_key() != volume_key);
        document.ensure_valid()?;
        Ok(document)
    }

    /// Decodes one bounded warm pointer document.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid data.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_PUBLISHED_WARM_BYTES {
            return Err(WireError::invalid(CONTRACT, "size"));
        }
        let document =
            serde_json::from_slice::<Self>(bytes).map_err(|_| WireError::decode(CONTRACT))?;
        document.ensure_valid()?;
        Ok(document)
    }

    /// Encodes one bounded warm pointer document.
    ///
    /// # Errors
    /// Returns an error when validation or serialization fails.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.ensure_valid()?;
        let bytes = serde_json::to_vec(self).map_err(|_| WireError::encode(CONTRACT))?;
        if bytes.len() > MAX_PUBLISHED_WARM_BYTES {
            return Err(WireError::invalid(CONTRACT, "size"));
        }
        Ok(bytes)
    }

    /// Applies the v1 document's structural validation.
    ///
    /// # Errors
    /// Returns the first structurally invalid field.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        super::validate::ensure_valid(self)
    }

    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    #[must_use]
    pub fn logical_name(&self) -> &str {
        &self.logical_name
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn produced_by(&self) -> Uuid {
        self.produced_by
    }

    #[must_use]
    pub const fn produced_at_unix(&self) -> u64 {
        self.produced_at_unix
    }

    #[must_use]
    pub const fn current(&self) -> &VolumePointer {
        &self.current
    }

    #[must_use]
    pub fn superseded(&self) -> &[VolumePointer] {
        &self.superseded
    }

    /// Every pointer this document names, current first.
    #[must_use]
    pub fn pointers(&self) -> Vec<&VolumePointer> {
        std::iter::once(&self.current)
            .chain(self.superseded.iter())
            .collect()
    }

    #[must_use]
    pub(super) const fn schema_version(&self) -> u8 {
        self.schema_version
    }
}
