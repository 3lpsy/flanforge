use std::{collections::BTreeSet, path::Path, time::Duration};

use flanforge_libvirt_wire::{CheckpointVolumeRole, VolumeCheckpoint};
use flanforge_manager::WorkerError;
use uuid::Uuid;

use crate::{
    RuntimeError,
    checkpoint::{
        load as load_checkpoint, remove as remove_checkpoint, warm_capture_dir, warm_capture_id,
        warm_path,
    },
    image::ImportJournal,
    worker::LibvirtWorker,
};

use super::pointer;

const RECOVERY_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_CAPTURES: usize = 4_096;

impl LibvirtWorker {
    /// Reconciles durable backend state once at daemon start.
    ///
    /// Acts only on the pair a pointer and a checkpoint form, never on name
    /// presence, and never promotes a candidate: it cannot know whether that
    /// candidate passed marker verification and identity generalization.
    pub(crate) async fn ensure_warm_recovered(&self) -> Result<(), WorkerError> {
        let referenced = self.referenced_volume_keys().await?;
        for capture_id in self.pending_captures().await? {
            self.reconcile_capture(capture_id, &referenced).await?;
        }
        self.ensure_imports_reconciled().await;
        Ok(())
    }

    /// Every volume any live pointer names, by key and by name.
    async fn referenced_volume_keys(&self) -> Result<BTreeSet<String>, WorkerError> {
        let state_dir = self.state_dir.clone();
        let documents = tokio::task::spawn_blocking(move || pointer::load_all(&state_dir))
            .await
            .map_err(|_| WorkerError::new("warm pointer load task failed"))??;
        Ok(documents
            .iter()
            .flat_map(|document| document.pointers().into_iter())
            .flat_map(|pointer| {
                [
                    pointer.volume_key().to_owned(),
                    pointer.volume_name().to_owned(),
                ]
            })
            .collect())
    }

    async fn pending_captures(&self) -> Result<Vec<Uuid>, WorkerError> {
        let directory = warm_capture_dir(&self.state_dir);
        tokio::task::spawn_blocking(move || capture_ids(&directory))
            .await
            .map_err(|_| WorkerError::new("warm capture scan task failed"))?
            .map_err(Into::into)
    }

    /// A checkpoint whose volume a pointer names means publication completed,
    /// so only the checkpoint goes. Anything else is an interrupted capture:
    /// an unpublished generation can be referenced by nothing, by
    /// construction, so it is deleted by exact name and, when one was
    /// recorded, by key.
    async fn reconcile_capture(
        &self,
        capture_id: Uuid,
        referenced: &BTreeSet<String>,
    ) -> Result<(), WorkerError> {
        let path = warm_path(&self.state_dir, capture_id);
        let checkpoint = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || load_checkpoint(&path))
                .await
                .map_err(|_| WorkerError::new("warm checkpoint task failed"))??
        };
        let Some(checkpoint) = checkpoint else {
            return Ok(());
        };
        let name = VolumeCheckpoint::warm_volume_name(capture_id);
        let key = checkpoint
            .volume(CheckpointVolumeRole::Warm)
            .map(|volume| volume.key().to_owned());
        let is_published =
            referenced.contains(&name) || key.as_ref().is_some_and(|key| referenced.contains(key));
        if !is_published {
            tracing::warn!(%capture_id, "removing an interrupted warm capture");
            self.actor
                .delete_volume(key, name, RECOVERY_TIMEOUT)
                .await?;
        }
        tokio::task::spawn_blocking(move || remove_checkpoint(&path))
            .await
            .map_err(|_| WorkerError::new("warm checkpoint removal task failed"))??;
        Ok(())
    }

    /// The import journal is only ever reachable from the operator CLI, so a
    /// crash mid-import left checkpoints nobody reconciled. A journal another
    /// process already holds is left to it.
    async fn ensure_imports_reconciled(&self) {
        let state_dir = self.state_dir.clone();
        let journal = tokio::task::spawn_blocking(move || ImportJournal::open(&state_dir)).await;
        let journal = match journal {
            Ok(Ok(journal)) => journal,
            Ok(Err(RuntimeError::ActorUnavailable)) => {
                tracing::debug!("import journal is held elsewhere; skipping reconciliation");
                return;
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "import journal is unavailable");
                return;
            }
            Err(_) => return,
        };
        if let Err(error) = crate::worker::reconcile_pending(
            &journal,
            &self.actor,
            &self.image_manifest_dir,
            self.instance.id(),
            self.actor.pool(),
            &self.state_dir,
        )
        .await
        {
            tracing::error!(%error, "pending image imports were not reconciled");
        }
    }
}

fn capture_ids(directory: &Path) -> Result<Vec<Uuid>, RuntimeError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    .map_err(RuntimeError::manifest)?;
    if entries.len() > MAX_CAPTURES {
        return Err(RuntimeError::manifest(
            "warm capture state has too many entries",
        ));
    }
    Ok(entries
        .iter()
        .filter_map(|entry| entry.file_name().to_str().and_then(warm_capture_id))
        .collect())
}
