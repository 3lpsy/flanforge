use flanforge_core::{
    Allocation, ProfileName, VmName, WarmGeneration, WarmImageRecord, WarmImageState,
};

use super::super::{AllocationManager, ManagerError};

/// What retention may write, re-read from the current configuration and the
/// current image record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionPlan {
    pub warm_template: VmName,
    pub generation: u64,
    pub previous: Option<WarmGeneration>,
}

impl AllocationManager {
    pub(crate) async fn record_warm_image(
        &self,
        record: WarmImageRecord,
    ) -> Result<(), ManagerError> {
        if record.state == WarmImageState::Promoted {
            self.inner.quarantined.lock().await.remove(&record.profile);
        }
        self.inner.images.save(&record).await?;
        Ok(())
    }

    /// Rewrites a profile's warm record as the generation that survives an
    /// abandoned promotion, or drops it when none does.
    ///
    /// A record left describing a candidate that was never promoted costs
    /// every later allocation a cold boot, so both the backend rollback and
    /// startup recovery come through here.
    ///
    /// # Errors
    /// Returns an error when the record cannot be written or removed.
    pub(crate) async fn ensure_warm_reverted(
        &self,
        profile: &ProfileName,
        warm_template: &VmName,
        previous: Option<&WarmGeneration>,
    ) -> Result<(), ManagerError> {
        match previous {
            Some(previous) => {
                self.inner
                    .images
                    .save(&WarmImageRecord {
                        profile: profile.clone(),
                        warm_template: warm_template.clone(),
                        generation: previous.generation,
                        base_fingerprint: previous.base_fingerprint.clone(),
                        produced_by: previous.produced_by,
                        produced_at_unix: previous.produced_at_unix,
                        state: WarmImageState::Promoted,
                        previous: None,
                    })
                    .await?;
            }
            None => self.inner.images.remove(profile).await?,
        }
        Ok(())
    }
}

/// Re-reads production authority at retention time: the profile must still
/// declare it, the allocation must have cloned the trusted base, and the warm
/// name must not be an image the daemon never claimed.
pub(crate) async fn retention_plan(
    manager: &AllocationManager,
    allocation: &Allocation,
) -> Result<Option<RetentionPlan>, ManagerError> {
    if !allocation.is_retention_eligible() {
        return Ok(None);
    }
    let config = manager.inner.config.current();
    let name = &allocation.request.profile;
    let Some(profile) = config.profiles.get(name) else {
        return Ok(None);
    };
    let Some(warm) = profile.warm_template.as_ref() else {
        return Ok(None);
    };
    if profile.regeneration_workflow.is_none() {
        return Ok(None);
    }
    // Lineage stays one generation deep: only a clone of the trusted base may
    // become the next image.
    if allocation
        .source
        .as_ref()
        .is_none_or(|source| source.name != profile.template)
    {
        tracing::warn!(allocation_id = %allocation.id, "retention refused: the guest was not cloned from the profile template");
        return Ok(None);
    }
    let record = manager.inner.images.load(name).await?;
    let listing = manager.host_machines().await?;
    if record.is_none()
        && manager
            .inner
            .worker
            .is_unclaimed_image(warm, &listing)
            .await?
    {
        tracing::error!(allocation_id = %allocation.id, warm_template = %warm, "retention refused: the warm name is an unclaimed pre-existing VM");
        return Ok(None);
    }
    Ok(Some(RetentionPlan {
        warm_template: warm.clone(),
        generation: record.as_ref().map_or(1, |record| record.generation + 1),
        previous: record.map(|record| WarmGeneration {
            generation: record.generation,
            base_fingerprint: record.base_fingerprint,
            produced_by: record.produced_by,
            produced_at_unix: record.produced_at_unix,
        }),
    }))
}
