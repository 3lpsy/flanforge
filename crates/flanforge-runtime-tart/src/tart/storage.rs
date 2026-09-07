use flanforge_core::Allocation;

use super::TartClient;

/// Tart keeps one disk per VM directory, whatever format `config.json`
/// declares for it.
pub(super) const DISK_IMAGE: &str = "disk.img";

const MIB: u64 = 1_048_576;

/// `tart set --disk-size` counts decimal gigabytes, and Tart only ever grows a
/// disk: a size that is not strictly larger than the current one is refused.
const DISK_SIZE_BYTES: u64 = 1_000_000_000;

impl TartClient {
    /// The virtual size of a machine's disk, in bytes.
    ///
    /// Tart truncates a raw image to exactly its virtual size, so the file
    /// length is that size. None where no library can be derived or it holds
    /// no such machine, which leaves the disk at whatever it already is.
    pub(crate) async fn disk_bytes(&self, name: &str) -> Option<u64> {
        let directory = self.library_vm_directory(name)?;
        tokio::fs::metadata(directory.join(DISK_IMAGE))
            .await
            .ok()
            .map(|metadata| metadata.len())
    }

    /// A base's virtual size in MiB, rounded down: Tart resizes only upward, so
    /// a rounded-up floor would grow every clone by a gigabyte to satisfy a
    /// sub-MiB remainder.
    pub(crate) async fn base_storage_mb(&self, name: &str) -> Option<u64> {
        self.disk_bytes(name).await.map(|bytes| bytes / MIB)
    }

    /// The `--disk-size` argument for this clone, or None to leave it at the
    /// size it inherited from its base.
    ///
    /// A profile asking for less than its base is warned about and run at the
    /// base size: a Tart disk cannot shrink, and the guest boots fine on the
    /// larger one.
    pub(crate) async fn disk_size_gb(
        &self,
        allocation: &Allocation,
        storage_mb: u64,
    ) -> Option<u16> {
        let Some(base_bytes) = self.disk_bytes(allocation.vm_name.as_str()).await else {
            tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, storage_mb, "no Tart disk can be measured for this clone, so its size is left alone");
            return None;
        };
        let base_mb = base_bytes / MIB;
        if storage_mb < base_mb {
            tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, requested_mb = storage_mb, base_mb, effective_mb = base_mb, "profile storage is smaller than the cloned base; the guest keeps the base disk");
        }
        let argument = disk_size_argument(storage_mb.saturating_mul(MIB), base_bytes);
        if argument.is_none() && storage_mb > base_mb {
            tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, requested_mb = storage_mb, base_mb, "the requested guest disk is larger than Tart can be asked for, so its size is left alone");
        }
        argument
    }
}

/// Rounds the effective disk up to whole gigabytes, and omits the argument
/// whenever the base already covers the request.
pub(crate) fn disk_size_argument(requested_bytes: u64, base_bytes: u64) -> Option<u16> {
    if requested_bytes <= base_bytes {
        return None;
    }
    u16::try_from(requested_bytes.div_ceil(DISK_SIZE_BYTES)).ok()
}
