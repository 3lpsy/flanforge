use std::time::Duration;

use flanforge_core::{LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS, WarmImageRecord, WarmImageState};
use flanforge_libvirt_wire::{CheckpointVolumeRole, VolumeCheckpoint};
use flanforge_manager::{AllocationReporter, WorkerError};
use uuid::Uuid;

use crate::{
    checkpoint::{remove as remove_checkpoint, warm_path},
    worker::LibvirtWorker,
};

use super::{Stopped, WarmRequest, plan::unix_time};

impl LibvirtWorker {
    /// Deletes the candidate by exact name, and by key when one was recorded,
    /// and puts a staged record back to the generation that survives.
    ///
    /// No pointer is restored: the live pointer never moved and the live volume
    /// was never touched. The record is another matter — left in `Staging` it
    /// costs every later allocation a cold boot, and nothing but a restart
    /// repairs it.
    pub(super) async fn rollback(
        &self,
        request: &WarmRequest<'_>,
        reporter: &AllocationReporter,
        capture_id: Uuid,
        stopped: &Stopped,
    ) {
        if stopped.is_record_staged
            && let Err(error) = reporter
                .ensure_warm_reverted(
                    request.profile,
                    &request.warm_template,
                    request.previous.as_ref(),
                )
                .await
        {
            tracing::error!(profile = %request.profile, %error, "cannot revert the staged warm image record");
        }
        let name = VolumeCheckpoint::warm_volume_name(capture_id);
        let path = warm_path(&self.state_dir, capture_id);
        let key = tokio::task::spawn_blocking(move || crate::checkpoint::load(&path))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|checkpoint| {
                checkpoint
                    .volume(CheckpointVolumeRole::Warm)
                    .map(|volume| volume.key().to_owned())
            });
        let budget = Duration::from_secs(LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS);
        if let Err(error) = self.actor.delete_volume(key, name, budget).await {
            tracing::error!(%error, "warm capture rollback left a volume behind");
            return;
        }
        if let Err(error) = self.forget_capture(capture_id).await {
            tracing::error!(%error, "warm capture rollback left a checkpoint behind");
        }
    }

    pub(super) async fn forget_capture(&self, capture_id: Uuid) -> Result<(), WorkerError> {
        let path = warm_path(&self.state_dir, capture_id);
        tokio::task::spawn_blocking(move || remove_checkpoint(&path))
            .await
            .map_err(|_| WorkerError::new("warm checkpoint removal task failed"))?
            .map_err(Into::into)
    }

    pub(super) async fn record(
        &self,
        request: &WarmRequest<'_>,
        reporter: &AllocationReporter,
        state: WarmImageState,
    ) -> Result<(), WorkerError> {
        reporter
            .record_warm_image(WarmImageRecord {
                profile: request.profile.clone(),
                warm_template: request.warm_template.clone(),
                generation: request.generation,
                base_fingerprint: request.base_fingerprint.clone(),
                produced_by: request.allocation.id,
                produced_at_unix: unix_time(),
                state,
                previous: request.previous.clone(),
            })
            .await
            .map_err(|error| WorkerError::new(error.to_string()))
    }
}
