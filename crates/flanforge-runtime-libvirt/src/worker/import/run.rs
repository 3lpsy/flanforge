use std::{path::Path, time::Duration};

use flanforge_core::{Config, VmName};
use flanforge_libvirt_wire::{HelperFailureCode, PublishedBase};
use flanforge_store::StateMutationLock;

use crate::{
    RuntimeError,
    actor::{ImportRequest, LibvirtActor},
    checkpoint::{import_path, remove as remove_checkpoint},
    context::{actor_config, backend, ensure_lock},
    image::{ImportJournal, inspect_qcow2, publish, publish_replace, stage, verify_image},
    manifest::ServiceInstance,
};

use super::recovery::{ensure_checkpoint_matches, reconcile_pending};

/// Verifies and imports one immutable base image into the configured pool.
/// `replace` republishes an already-published name in place — the startup
/// auto-import's supersede arm; the CLI never passes it.
///
/// # Errors
/// Returns an error when verification, helper execution, publication, or crash
/// reconciliation cannot complete without exact ownership proof.
pub async fn import_base(
    config: &Config,
    lock: &StateMutationLock,
    logical_name: &VmName,
    image_path: &Path,
    manifest_path: Option<&Path>,
    replace: bool,
) -> Result<PublishedBase, RuntimeError> {
    let backend = backend(config)?;
    ensure_lock(config, lock)?;
    let journal = ImportJournal::open(&config.runtime.state_dir)?;
    let image = verify_image(image_path, manifest_path, &backend.qemu_img_path).await?;
    let state_dir = config.runtime.state_dir.clone();
    let instance = tokio::task::spawn_blocking(move || ServiceInstance::load_or_create(&state_dir))
        .await
        .map_err(|_| RuntimeError::manifest("service-instance task failed"))??;
    let actor = LibvirtActor::open(actor_config(config, instance.id())?).await?;
    reconcile_pending(
        &journal,
        &actor,
        &backend.image_manifest_dir,
        instance.id(),
        &backend.pool,
        &config.runtime.state_dir,
    )
    .await?;
    let staged = stage(image, &config.runtime.state_dir).await?;
    let intent = journal.record(logical_name.clone(), &staged)?;
    if let Err(error) = inspect_qcow2(&staged.path, &backend.qemu_img_path, &staged.manifest).await
    {
        let _ = journal.finish(&intent);
        return Err(error);
    }
    let publication = actor
        .import(
            ImportRequest {
                logical_name: logical_name.to_string(),
                volume_name: staged.volume_name.clone(),
                staged_image_path: staged.path.clone(),
                manifest: staged.manifest.clone(),
            },
            Duration::from_mins(10),
        )
        .await;
    let publication = match publication {
        Ok(publication) => publication,
        Err(error) => {
            handle_import_error(config, &journal, &intent, &error);
            return Err(error);
        }
    };
    let intent = match journal.mark_created(&intent, publication.volume_key().to_owned()) {
        Ok(intent) => intent,
        Err(error) => {
            let cleanup = actor
                .delete_volume(
                    Some(publication.volume_key().to_owned()),
                    publication.volume_name().to_owned(),
                    Duration::from_secs(30),
                )
                .await;
            if cleanup.is_ok() {
                let _ =
                    remove_checkpoint(&import_path(&config.runtime.state_dir, intent.import_id()));
                let _ = journal.finish(&intent);
            }
            return Err(error);
        }
    };
    ensure_checkpoint_matches(
        &intent,
        &publication,
        instance.id(),
        &backend.pool,
        &config.runtime.state_dir,
    )?;
    let published = if replace {
        publish_replace(&backend.image_manifest_dir, &publication)
    } else {
        publish(&backend.image_manifest_dir, &publication)
    };
    if let Err(error) = published {
        if error.is_committed() {
            return Err(error.into_runtime_error());
        }
        let cleanup = actor
            .delete_volume(
                Some(publication.volume_key().to_owned()),
                publication.volume_name().to_owned(),
                Duration::from_secs(30),
            )
            .await;
        if cleanup.is_ok() {
            remove_checkpoint(&import_path(&config.runtime.state_dir, intent.import_id()))?;
            let _ = journal.finish(&intent);
        }
        return Err(error.into_runtime_error());
    }
    remove_checkpoint(&import_path(&config.runtime.state_dir, intent.import_id()))?;
    journal.finish(&intent)?;
    Ok(publication)
}

fn handle_import_error(
    config: &Config,
    journal: &ImportJournal,
    intent: &crate::image::ImportIntent,
    error: &RuntimeError,
) {
    if matches!(
        error,
        RuntimeError::Helper {
            code: HelperFailureCode::Conflict,
            ..
        }
    ) {
        let checkpoint =
            remove_checkpoint(&import_path(&config.runtime.state_dir, intent.import_id()));
        if checkpoint.is_ok() {
            if let Err(finish_error) = journal.finish(intent) {
                tracing::error!(%finish_error, "collided import intent could not be retired");
            }
        } else if let Err(checkpoint_error) = checkpoint {
            tracing::error!(
                %checkpoint_error,
                "collided import checkpoint could not be retired; preserving the intent"
            );
        }
    } else {
        tracing::error!(
            volume_name = %intent.volume_name(),
            "libvirt import ownership is ambiguous; preserving the intent without deleting by name"
        );
    }
}
