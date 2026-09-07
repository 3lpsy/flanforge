use flanforge_core::{CloneSource, GuestSize, ProfileName};

use super::super::AllocationManager;

impl AllocationManager {
    /// The sizing admission charges and the allocation records: the resolved
    /// request, floored at the virtual size of the base the guest boots from.
    ///
    /// An ephemeral guest's disk is `max(storage_mb, base virtual size)` on
    /// every backend, so charging the declared value alone under-commits the
    /// host budget whenever a base is larger than its profile.
    pub(crate) async fn effective_size(
        &self,
        size: GuestSize,
        profile: &ProfileName,
        source: &CloneSource,
    ) -> GuestSize {
        let Some(base_mb) = self.inner.worker.base_storage_mb(profile, source).await else {
            return size;
        };
        floored(size, base_mb)
    }
}

/// Raises `storage_mb` to the base's virtual size, clamped to the validated
/// ceiling so the result is always a recordable size.
pub(crate) fn floored(size: GuestSize, base_mb: u64) -> GuestSize {
    let storage_mb = size.storage_mb.max(base_mb.min(GuestSize::MAX_STORAGE_MB));
    if storage_mb > size.storage_mb {
        tracing::warn!(
            requested_mb = size.storage_mb,
            base_mb,
            effective_mb = storage_mb,
            "profile storage is smaller than the boot source; charging the base size"
        );
    }
    GuestSize { storage_mb, ..size }
}
