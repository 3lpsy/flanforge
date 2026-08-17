use flanforge_core::{Allocation, VmName, WarmGeneration, WarmImageRecord, WarmImageState};

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
}

/// Re-reads production authority at retention time: the profile must still
/// declare it, the allocation must have cloned the trusted base, and the warm
/// name must not be an image the daemon never claimed.
pub(crate) async fn retention_plan(
    manager: &AllocationManager,
    allocation: &Allocation,
) -> Option<RetentionPlan> {
    if !allocation.is_retention_eligible() {
        return None;
    }
    let config = manager.inner.config.current();
    let name = &allocation.request.profile;
    let profile = config.profiles.get(name)?;
    let warm = profile.warm_template.as_ref()?;
    profile.regeneration_workflow.as_ref()?;
    // Lineage stays one generation deep: only a clone of the trusted base may
    // become the next image.
    if allocation
        .source
        .as_ref()
        .is_none_or(|source| source.name != profile.template)
    {
        tracing::warn!(allocation_id = %allocation.id, "retention refused: the guest was not cloned from the profile template");
        return None;
    }
    let record = manager.inner.images.load(name).await.ok().flatten();
    if record.is_none() && is_present(manager, warm).await {
        tracing::error!(allocation_id = %allocation.id, warm_template = %warm, "retention refused: the warm name is an unclaimed pre-existing VM");
        return None;
    }
    Some(RetentionPlan {
        warm_template: warm.clone(),
        generation: record.as_ref().map_or(1, |record| record.generation + 1),
        previous: record.map(|record| WarmGeneration {
            generation: record.generation,
            base_fingerprint: record.base_fingerprint,
            produced_by: record.produced_by,
            produced_at_unix: record.produced_at_unix,
        }),
    })
}

async fn is_present(manager: &AllocationManager, name: &VmName) -> bool {
    manager
        .inner
        .worker
        .machines()
        .await
        .unwrap_or_default()
        .iter()
        .any(|machine| machine.name == name.as_str())
}
