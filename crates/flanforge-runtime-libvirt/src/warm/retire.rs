use std::{collections::BTreeMap, time::Duration};

use flanforge_core::{ProfileName, VmName};
use flanforge_libvirt_wire::{PublishedWarm, VolumePointer};
use flanforge_manager::{ImageSweep, RetiredImage, WorkerError};

use crate::{RuntimeError, worker::LibvirtWorker};

use super::{capture::unix_time, pointer};

/// A UUID-named warm volume is never nameable in configuration, so the
/// reaper's longer image floor — which exists so an in-progress config edit
/// cannot destroy the image a profile is being repointed at — does not apply.
pub(super) const WARM_RETIREMENT_AGE_SECONDS: u64 = 3_600;

/// Bounds one mutex-held retirement so a slow remote pool cannot hold the
/// ordering lock for anything like a capture budget.
const RETIREMENT_TIMEOUT: Duration = Duration::from_mins(2);

const DRY_RUN: &str = "dry run; nothing was deleted";
const RETIRED: &str = "proven unreferenced and deleted";
const PINNED: &str = "pinned by a live or recoverable overlay";
const TOO_YOUNG: &str = "younger than the retirement age floor";

impl LibvirtWorker {
    /// Retires superseded warm generations the reference proof clears.
    ///
    /// An unreferenced profile's `current` becomes a candidate only once its
    /// superseded entries are all gone, so the pointer document is valid after
    /// every pass and the collection completes over at most two sweeps.
    pub(crate) async fn ensure_superseded_retired(
        &self,
        request: ImageSweep<'_>,
    ) -> Result<Vec<RetiredImage>, WorkerError> {
        // Held across the whole sweep, and every document is read inside it:
        // a promotion that landed mid-sweep would otherwise be overwritten by
        // the pre-image this pass started from, stranding its volume.
        let _guard = self.pointer_guard().await;
        let state_dir = self.state_dir.clone();
        let documents = tokio::task::spawn_blocking(move || pointer::load_all(&state_dir))
            .await
            .map_err(|_| WorkerError::new("warm pointer load task failed"))??;
        let protected = self.protected_pointers(&documents).await?;
        let now = unix_time();
        let mut reported = Vec::new();
        for document in &documents {
            let Ok(profile) = ProfileName::new(document.profile()) else {
                continue;
            };
            let candidates = candidates(document, &request, &profile);
            let (aged, young): (Vec<_>, Vec<_>) = candidates
                .into_iter()
                .partition(|pointer| is_old_enough(pointer, now));
            reported.extend(
                young
                    .iter()
                    .map(|pointer| report(&profile, pointer, now, false, TOO_YOUNG)),
            );
            if aged.is_empty() {
                continue;
            }
            if request.is_dry_run {
                reported.extend(
                    aged.iter()
                        .map(|pointer| report(&profile, pointer, now, false, DRY_RUN)),
                );
                continue;
            }
            let protected = protected
                .iter()
                .filter(|pointer| {
                    !aged
                        .iter()
                        .any(|candidate| candidate.is_same_volume(pointer))
                })
                .cloned()
                .collect::<Vec<_>>();
            let deleted = self
                .actor
                .warm_retire(aged.clone(), protected, RETIREMENT_TIMEOUT)
                .await?;
            for pointer in &aged {
                let is_deleted = deleted.iter().any(|key| key == pointer.volume_key());
                reported.push(report(
                    &profile,
                    pointer,
                    now,
                    is_deleted,
                    if is_deleted { RETIRED } else { PINNED },
                ));
            }
            // Volume first, then the pointer entry: a crash between them
            // leaves an entry naming a deleted volume, which the next sweep
            // drops when its lookup fails. The reverse orphans a full-size
            // volume that can never again be proven ours.
            self.ensure_pointer_updated(document, &profile, &deleted)
                .await?;
        }
        Ok(reported)
    }

    /// Superseded generations still pinned, from durable state only.
    pub(crate) async fn retained_generations(&self, profile: &ProfileName) -> Option<u32> {
        let document = self.load_pointer(profile).await.ok()??;
        u32::try_from(document.superseded().len()).ok()
    }

    /// Every generation this retirement may not touch: each profile's live
    /// pointer and, for any other profile, its superseded entries too, plus
    /// every published cold base.
    async fn protected_pointers(
        &self,
        documents: &[PublishedWarm],
    ) -> Result<Vec<VolumePointer>, WorkerError> {
        let mut protected = documents
            .iter()
            .flat_map(|document| document.pointers().into_iter().cloned())
            .collect::<Vec<_>>();
        let directory = self.image_manifest_dir.clone();
        let bases = tokio::task::spawn_blocking(move || published_base_pointers(&directory))
            .await
            .map_err(|_| WorkerError::new("published base scan task failed"))??;
        protected.extend(bases);
        Ok(protected)
    }

    async fn ensure_pointer_updated(
        &self,
        document: &PublishedWarm,
        profile: &ProfileName,
        deleted: &[String],
    ) -> Result<(), WorkerError> {
        if deleted.is_empty() {
            return Ok(());
        }
        let is_current_gone = deleted
            .iter()
            .any(|key| key == document.current().volume_key());
        let state_dir = self.state_dir.clone();
        let profile = profile.clone();
        let document = document.clone();
        let deleted = deleted.to_vec();
        tokio::task::spawn_blocking(move || {
            if is_current_gone {
                return pointer::remove(&state_dir, &profile);
            }
            let mut updated = document;
            for key in &deleted {
                updated = updated
                    .without_superseded(key)
                    .map_err(RuntimeError::manifest)?;
            }
            pointer::ensure_published(&state_dir, &updated)
        })
        .await
        .map_err(|_| WorkerError::new("warm pointer update task failed"))?
        .map_err(Into::into)
    }
}

/// Superseded entries always; `current` only when no live profile declares the
/// pointer's logical name, no record claims the profile, and every superseded
/// entry has already been collected.
pub(super) fn candidates(
    document: &PublishedWarm,
    request: &ImageSweep<'_>,
    profile: &ProfileName,
) -> Vec<VolumePointer> {
    let mut candidates = document.superseded().to_vec();
    let is_declared = request
        .declared
        .get(profile)
        .is_some_and(|warm: &VmName| warm.as_str() == document.logical_name());
    if candidates.is_empty() && !is_declared && !request.claimed.contains(profile) {
        candidates.push(document.current().clone());
    }
    candidates
}

pub(super) fn is_old_enough(pointer: &VolumePointer, now: u64) -> bool {
    pointer.produced_at_unix() != 0
        && now.saturating_sub(pointer.produced_at_unix()) >= WARM_RETIREMENT_AGE_SECONDS
}

fn report(
    profile: &ProfileName,
    pointer: &VolumePointer,
    now: u64,
    is_deleted: bool,
    reason: &str,
) -> RetiredImage {
    RetiredImage {
        profile: profile.clone(),
        generation: pointer.generation(),
        age_seconds: now.saturating_sub(pointer.produced_at_unix()),
        is_deleted,
        reason: reason.to_owned(),
    }
}

/// A bounded read of the publication directory, so a cold base can never be a
/// retirement candidate however a pointer document is edited.
fn published_base_pointers(
    directory: &std::path::Path,
) -> Result<Vec<VolumePointer>, RuntimeError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    .map_err(RuntimeError::manifest)?;
    if entries.len() > 4_096 {
        return Err(RuntimeError::manifest(
            "published base directory has too many entries",
        ));
    }
    let mut pointers = BTreeMap::new();
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Some(logical) = name.strip_suffix(".published.json") else {
            continue;
        };
        let Ok(logical) = VmName::new(logical) else {
            continue;
        };
        let base = crate::image::load_published(directory, &logical)?;
        pointers.insert(
            base.volume_name().to_owned(),
            base.pointer().map_err(RuntimeError::manifest)?,
        );
    }
    Ok(pointers.into_values().collect())
}
