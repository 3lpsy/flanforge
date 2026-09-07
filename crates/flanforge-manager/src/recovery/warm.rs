use std::time::Duration;

use flanforge_core::{
    Config, Profile, ProfileName, RetentionOutcome, RetentionPhase, RetentionResult, VmName,
    WarmImageRecord, WarmImageState,
};

use super::super::{
    AllocationManager, ManagerError, ReapRequest, WarmAvailability,
    reaper::{ReapAuthorization, reserved_image_names},
};

impl AllocationManager {
    /// Reconciles each declaring profile's warm image against the backend.
    /// Recovery never promotes a staged candidate: it cannot know whether that
    /// candidate passed its backend's pre-capture gates.
    pub(super) async fn ensure_warm_consistent(&self, config: &Config) -> Result<(), ManagerError> {
        let machines = self.inner.worker.machines().await?;
        let is_present =
            |name: &VmName| machines.iter().any(|machine| machine.name == name.as_str());
        for (name, profile) in &config.profiles {
            if profile.warm_template.is_none() {
                continue;
            }
            let Some(record) = self.inner.images.load(name).await? else {
                continue;
            };
            let (Ok(staging), Ok(_)) = (record.staging_name(), record.previous_name()) else {
                tracing::error!(profile = %name, "warm image names are underivable; skipping reconciliation");
                continue;
            };
            let warm_present = match self
                .inner
                .worker
                .warm_availability(profile, &record, &machines)
                .await
            {
                Ok(availability) => {
                    matches!(
                        availability,
                        WarmAvailability::Ready | WarmAvailability::Busy
                    )
                }
                Err(error) => {
                    tracing::error!(profile = %name, %error, "warm image availability is unreadable; skipping reconciliation");
                    continue;
                }
            };
            // Dead by construction on a pointer-addressed backend: its listing
            // reports domains, and a warm generation is a volume. Do not
            // "fix" that by synthesizing machine entries for volumes — it
            // would drag them into the sweep's prefix and age reasoning, which
            // a volume has no counterpart to.
            let staging_present = is_present(&staging);
            if warm_present && staging_present && record.state == WarmImageState::Promoted {
                tracing::warn!(profile = %name, "promotion was interrupted after it completed; removing the candidate");
            } else if warm_present && record.state == WarmImageState::Staging && !staging_present {
                tracing::warn!(profile = %name, generation = record.generation, "promotion finished but was never recorded; finalizing");
                self.finalize_record(&record).await?;
            } else if warm_present && record.state == WarmImageState::Staging {
                // The candidate is still there, so retirement never ran and the
                // live image is the surviving generation: only the record moves.
                tracing::warn!(profile = %name, generation = record.generation, "promotion was interrupted before anything was retired; reverting the record");
                if let Err(error) = self.revert_record(&record).await {
                    tracing::error!(profile = %name, %error, "cannot revert the warm image record");
                }
            } else if !warm_present {
                self.restore_warm(name, profile, &record).await;
            }
            if staging_present {
                self.delete_staged(name, profile, &record, &staging).await;
            }
            if record.state == WarmImageState::Staging {
                self.mark_retention_interrupted(&record).await;
            }
        }
        Ok(())
    }

    /// Rolls the record and the image back to the surviving generation. Only
    /// reached with the live image unusable, so a name-addressed restore
    /// overwrites nothing; the record reverts either way, because a record
    /// outliving its image costs a cold boot while the reverse is a silent
    /// stale one.
    async fn restore_warm(&self, name: &ProfileName, profile: &Profile, record: &WarmImageRecord) {
        if let Err(error) = self
            .inner
            .worker
            .ensure_warm_restored(profile, record)
            .await
        {
            tracing::error!(profile = %name, %error, "cannot restore the previous warm image");
        }
        if let Err(error) = self.revert_record(record).await {
            tracing::error!(profile = %name, %error, "cannot revert the warm image record");
        }
    }

    async fn delete_staged(
        &self,
        name: &ProfileName,
        profile: &Profile,
        record: &WarmImageRecord,
        staging: &VmName,
    ) {
        let authorization = ReapAuthorization::Staging(name.clone());
        let reserved = reserved_image_names(&self.inner.config.current());
        if let Err(error) = self
            .inner
            .worker
            .delete_vm(ReapRequest {
                name: staging,
                authorization: &authorization,
                profile: Some(profile),
                record: Some(record),
                // A staging image has no allocation and no registration.
                allocation: None,
                reserved: &reserved,
                budget: self.teardown_budget(Duration::from_secs(profile.cleanup_timeout_seconds)),
            })
            .await
        {
            tracing::warn!(profile = %name, %error, "cannot remove the interrupted staging image");
        }
    }

    async fn finalize_record(&self, record: &WarmImageRecord) -> Result<(), ManagerError> {
        let mut finalized = record.clone();
        finalized.state = WarmImageState::Promoted;
        self.inner.images.save(&finalized).await?;
        Ok(())
    }

    async fn revert_record(&self, record: &WarmImageRecord) -> Result<(), ManagerError> {
        self.ensure_warm_reverted(
            &record.profile,
            &record.warm_template,
            record.previous.as_ref(),
        )
        .await
    }

    /// Keeps the producing allocation's history honest about the interruption.
    async fn mark_retention_interrupted(&self, record: &WarmImageRecord) {
        let outcome = RetentionOutcome::new(
            RetentionResult::Failed,
            RetentionPhase::Promote,
            "the daemon restarted during promotion",
            Some(record.generation),
        );
        if let Err(error) = self.set_retention(record.produced_by, outcome).await {
            tracing::debug!(allocation_id = %record.produced_by, %error, "producing allocation is no longer loaded");
        }
    }
}
