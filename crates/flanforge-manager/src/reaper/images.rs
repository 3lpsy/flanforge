use std::collections::{BTreeMap, BTreeSet};

use flanforge_core::{ProfileName, WarmImageRecord};

use super::super::{AllocationManager, HostMachine, ImageSweep, RetiredImage};

/// What one pass decided about the warm image records, before anything is
/// applied.
#[derive(Debug, Default)]
pub(super) struct PrunePlan {
    pub(super) prunable: Vec<ProfileName>,
    pub(super) claimed: BTreeSet<ProfileName>,
}

impl AllocationManager {
    /// Which records a real run would drop, and which profiles still hold one
    /// afterwards. Computed once per pass, so a dry run considers exactly the
    /// candidates a real run would.
    ///
    /// Both authorities are the ones the pass already read: an empty `claimed`
    /// set tells retirement that no profile claims anything, so it may only
    /// ever be derived from records that were actually read. The configuration
    /// is re-read, because the sweep it follows may have run for minutes.
    pub(super) async fn plan_pruned_records(
        &self,
        images: &[WarmImageRecord],
        machines: &[HostMachine],
    ) -> PrunePlan {
        let config = self.inner.config.current();
        let regenerating = self.regenerating_profiles().await;
        let mut plan = PrunePlan::default();
        for record in images {
            let is_referenced = config
                .profiles
                .get(&record.profile)
                .and_then(|profile| profile.warm_template.as_ref())
                .is_some_and(|warm| warm == &record.warm_template);
            if !is_referenced
                && !is_present(record, machines)
                && !regenerating.contains(&record.profile)
            {
                plan.prunable.push(record.profile.clone());
            } else {
                plan.claimed.insert(record.profile.clone());
            }
        }
        plan
    }

    /// Drops a record once no image it names survives, no live profile still
    /// points at it, and no regeneration is mid-promotion on that profile.
    pub(super) async fn apply_pruned_records(&self, prunable: &[ProfileName]) -> Vec<ProfileName> {
        let mut pruned = Vec::new();
        for profile in prunable {
            if let Err(error) = self.inner.images.remove(profile).await {
                tracing::warn!(%profile, %error, "cannot remove an orphaned warm image record");
                continue;
            }
            tracing::info!(%profile, "reaper dropped a warm image record whose images are all gone");
            pruned.push(profile.clone());
        }
        pruned
    }

    /// Retires superseded warm generations the backend can prove unreferenced.
    /// Runs after the prune is decided, so a pointer whose profile stopped
    /// declaring `warm_template` is collected in the pass that drops its
    /// record — on a dry run too, which reports without applying either.
    pub(super) async fn sweep_warm_images(
        &self,
        is_dry_run: bool,
        claimed: &BTreeSet<ProfileName>,
    ) -> Vec<RetiredImage> {
        let config = self.inner.config.current();
        let declared = config
            .profiles
            .iter()
            .filter_map(|(name, profile)| {
                profile
                    .warm_template
                    .as_ref()
                    .map(|warm| (name.clone(), warm.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        match self
            .inner
            .worker
            .sweep_images(ImageSweep {
                is_dry_run,
                declared: &declared,
                claimed,
            })
            .await
        {
            Ok(retired) => retired,
            Err(error) => {
                tracing::warn!(%error, "warm image retirement did not complete");
                Vec::new()
            }
        }
    }
}

fn is_present(record: &WarmImageRecord, machines: &[HostMachine]) -> bool {
    [
        record.warm_template.clone(),
        record
            .staging_name()
            .unwrap_or_else(|_| record.warm_template.clone()),
        record
            .previous_name()
            .unwrap_or_else(|_| record.warm_template.clone()),
    ]
    .iter()
    .any(|name| machines.iter().any(|machine| machine.name == name.as_str()))
}
