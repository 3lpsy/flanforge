use flanforge_core::ProfileName;
use flanforge_libvirt_wire::PublishedWarm;
use flanforge_manager::WorkerError;
use tokio::sync::MutexGuard;

use crate::worker::LibvirtWorker;

use super::pointer;

impl LibvirtWorker {
    /// Orders every read-modify-write of a pointer document.
    ///
    /// A holder must re-read the document it writes inside this guard: the
    /// three writers otherwise interleave a stale snapshot over a newer one,
    /// stranding a full-size volume no sweep and no recovery could collect.
    pub(crate) async fn pointer_guard(&self) -> MutexGuard<'_, ()> {
        self.pointer_lock.lock().await
    }

    pub(crate) async fn load_pointer(
        &self,
        profile: &ProfileName,
    ) -> Result<Option<PublishedWarm>, WorkerError> {
        let state_dir = self.state_dir.clone();
        let profile = profile.clone();
        tokio::task::spawn_blocking(move || pointer::load(&state_dir, &profile))
            .await
            .map_err(|_| WorkerError::new("warm pointer load task failed"))?
            .map_err(Into::into)
    }

    /// Replaces one profile's pointer atomically. Only ever called under
    /// `pointer_guard`.
    pub(crate) async fn ensure_pointer_published(
        &self,
        document: PublishedWarm,
    ) -> Result<(), WorkerError> {
        let state_dir = self.state_dir.clone();
        tokio::task::spawn_blocking(move || pointer::ensure_published(&state_dir, &document))
            .await
            .map_err(|_| WorkerError::new("warm pointer publish task failed"))?
            .map_err(Into::into)
    }
}
