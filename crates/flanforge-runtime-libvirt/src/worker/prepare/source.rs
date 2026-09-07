use flanforge_core::{Allocation, CloneKind, CloneSource, FallbackReason, Profile, ProfileName};
use flanforge_libvirt_wire::{PublishedWarm, VolumePointer};
use flanforge_manager::{AllocationReporter, WorkerError};

use super::super::LibvirtWorker;
use super::load_published_async;

const MIB: u64 = 1_048_576;

/// One of this profile's own two declared names, and nothing else: an
/// allocation record is durable state a guest's own run can outlive, so it
/// authorizes nothing by itself.
pub(super) fn is_declared_source(profile: &Profile, source: &CloneSource) -> bool {
    source.name == profile.template || profile.warm_template.as_ref() == Some(&source.name)
}

/// The generation a declared warm source resolves to, or the reason it
/// degrades to a cold boot rather than failing the allocation.
pub(super) fn warm_pointer_of(
    document: &PublishedWarm,
    requested: &CloneSource,
) -> Result<VolumePointer, FallbackReason> {
    if document.logical_name() != requested.name.as_str() {
        return Err(FallbackReason::Repointed);
    }
    Ok(document.current().clone())
}

/// The overlay's capacity: `max(storage_mb, base virtual size)`. A base is
/// immutable and a warm one is sized at capture, so a profile asking for less
/// is warned about and run at the base size rather than refused.
pub(super) fn effective_storage_bytes(
    allocation: &Allocation,
    storage_mb: u64,
    base_bytes: u64,
) -> Result<u64, WorkerError> {
    let requested_bytes = storage_mb
        .checked_mul(MIB)
        .ok_or_else(|| WorkerError::new("guest storage size overflows bytes"))?;
    if requested_bytes < base_bytes {
        tracing::warn!(allocation_id = %allocation.id, requested_bytes, base_bytes, effective_bytes = base_bytes, "profile storage is smaller than the immutable base; the guest keeps the base disk");
    }
    Ok(requested_bytes.max(base_bytes))
}

impl LibvirtWorker {
    /// Re-resolves the boot source at create time rather than trusting the
    /// decision admission made minutes ago, and re-validates it as an
    /// authorization check: `source.name` must be one of this profile's own
    /// two declared names, so a stale or tampered allocation record cannot
    /// direct a guest at an arbitrary published image.
    pub(super) async fn resolve_source(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
    ) -> Result<VolumePointer, WorkerError> {
        let requested = allocation
            .source
            .as_ref()
            .filter(|source| is_declared_source(profile, source));
        let Some(requested) = requested else {
            if allocation.source.is_some() {
                tracing::error!(allocation_id = %allocation.id, "allocation names an image this profile does not declare; booting cold");
            }
            return self.cold_pointer(profile).await;
        };
        if requested.kind != CloneKind::Warm {
            return self.cold_pointer(profile).await;
        }
        match self.warm_pointer(allocation, requested).await {
            Ok(pointer) => Ok(pointer),
            Err(reason) => {
                tracing::warn!(allocation_id = %allocation.id, ?reason, "warm pointer is unusable at create time; booting cold");
                let cold = self.cold_pointer(profile).await?;
                let fallback = CloneSource {
                    name: profile.template.clone(),
                    kind: CloneKind::Template,
                    base_fingerprint: requested.base_fingerprint.clone(),
                    fallback_reason: Some(reason),
                };
                if let Err(error) = reporter.set_source(fallback).await {
                    tracing::warn!(allocation_id = %allocation.id, %error, "cannot record the resolved clone source");
                }
                Ok(cold)
            }
        }
    }

    async fn warm_pointer(
        &self,
        allocation: &Allocation,
        requested: &CloneSource,
    ) -> Result<VolumePointer, FallbackReason> {
        let document = self
            .load_pointer(&allocation.request.profile)
            .await
            .map_err(|_| FallbackReason::Absent)?
            .ok_or(FallbackReason::Absent)?;
        warm_pointer_of(&document, requested)
    }

    /// The virtual size, in MiB, of the volume this source boots from: the
    /// published base for a cold boot, the current generation for a warm one.
    ///
    /// Rounded up, so the recorded `storage_mb` is exactly the capacity the
    /// overlay is created at — a warm capture declares that same size, and a
    /// pointer that disagreed with its volume would refuse every later boot.
    /// None where the authority cannot be read, which leaves admission
    /// charging the declared size.
    pub(crate) async fn source_storage_mb(
        &self,
        name: &ProfileName,
        source: &CloneSource,
    ) -> Option<u64> {
        let virtual_bytes = if source.kind == CloneKind::Warm {
            let document = self.load_pointer(name).await.ok()??;
            if document.logical_name() != source.name.as_str() {
                return None;
            }
            document.current().virtual_bytes()
        } else {
            load_published_async(self.image_manifest_dir.clone(), source.name.clone())
                .await
                .ok()?
                .manifest()
                .virtual_bytes()
        };
        Some(virtual_bytes.div_ceil(MIB))
    }

    async fn cold_pointer(&self, profile: &Profile) -> Result<VolumePointer, WorkerError> {
        let base =
            load_published_async(self.image_manifest_dir.clone(), profile.template.clone()).await?;
        base.pointer()
            .map_err(|error| WorkerError::new(error.to_string()))
    }
}
