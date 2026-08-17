use std::collections::BTreeSet;

use flanforge_core::{Allocation, AllocationMode, ProfileName, VmName, WarmImageRecord, unix_time};
use serde::{Deserialize, Serialize};

use super::{
    super::{AllocationManager, HostMachine, ManagerError, ReapRequest},
    plan::{ReapAuthorization, ReapCandidate, ReapInputs, plan_sweep, reserved_image_names},
};

/// Why a pass could plan nothing, so an empty report is not read as a clean
/// host when the daemon simply cannot tell.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepInertReason {
    /// No machine age is determinable, which is what an unset
    /// `runtime.tart_home` looks like from here.
    AgeUndeterminable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SweepReport {
    pub planned: Vec<ReapCandidate>,
    pub deleted: Vec<String>,
    pub skipped: Vec<String>,
    /// Warm image records dropped because nothing they name survives.
    #[serde(default)]
    pub pruned_records: Vec<ProfileName>,
    #[serde(default)]
    pub inert_reason: Option<SweepInertReason>,
    pub finished_at_unix: u64,
}

impl AllocationManager {
    /// Sweeps unreferenced clones and images; a dry run reports and deletes
    /// nothing, and does not replace the record of the last deleting pass.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot be listed or the image records
    /// cannot be read; a single failed deletion is reported as skipped.
    pub async fn sweep(&self, is_dry_run: bool) -> Result<SweepReport, ManagerError> {
        let config = self.inner.config.current();
        let images = self.inner.images.load_all().await?;
        let machines = self.inner.worker.machines().await?;
        let inert_reason = inert_reason(&machines);
        let planned = plan_sweep(ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &self.allocation_snapshot().await,
            images: &images,
        });
        tracing::info!(
            candidates = planned.len(),
            is_dry_run,
            ?inert_reason,
            "reaper sweep planned"
        );

        let mut deleted = Vec::new();
        let mut skipped = Vec::new();
        let mut pruned_records = Vec::new();
        if !is_dry_run {
            for candidate in &planned {
                match self.reap(candidate).await {
                    Ok(true) => deleted.push(candidate.name.clone()),
                    Ok(false) => skipped.push(candidate.name.clone()),
                    Err(error) => {
                        tracing::warn!(name = %candidate.name, %error, "reaper deletion failed");
                        skipped.push(candidate.name.clone());
                    }
                }
            }
            pruned_records = self.prune_image_records().await;
        }

        let report = SweepReport {
            planned,
            deleted,
            skipped,
            pruned_records,
            inert_reason,
            finished_at_unix: unix_time(),
        };
        tracing::info!(
            deleted = report.deleted.len(),
            skipped = report.skipped.len(),
            pruned_records = report.pruned_records.len(),
            "reaper sweep finished"
        );
        if !is_dry_run {
            *self.inner.last_sweep.lock().await = Some(report.clone());
        }
        Ok(report)
    }

    #[must_use]
    pub async fn last_sweep(&self) -> Option<SweepReport> {
        self.inner.last_sweep.lock().await.clone()
    }

    /// Deletes one candidate after re-planning against a fresh listing and the
    /// current configuration, so a name claimed since the plan was computed is
    /// left alone. This narrows the window rather than closing it: a reload can
    /// still land between the re-plan and the deletion.
    async fn reap(&self, candidate: &ReapCandidate) -> Result<bool, ManagerError> {
        if candidate.authorization == ReapAuthorization::Prefix {
            tracing::warn!(name = %candidate.name, age_seconds = candidate.age_seconds, "prefixed VM has no record and is reported, never deleted");
            return Ok(false);
        }
        let Ok(name) = VmName::new(candidate.name.clone()) else {
            return Ok(false);
        };
        let config = self.inner.config.current();
        let images = self.inner.images.load_all().await?;
        let fresh = plan_sweep(ReapInputs {
            config: &config,
            machines: &self.inner.worker.machines().await?,
            allocations: &self.allocation_snapshot().await,
            images: &images,
        });
        if !fresh.iter().any(|current| {
            current.name == candidate.name && current.authorization == candidate.authorization
        }) {
            tracing::info!(name = %candidate.name, "reaper candidate was claimed before deletion");
            return Ok(false);
        }
        let profile_name = match &candidate.authorization {
            ReapAuthorization::Staging(profile) | ReapAuthorization::Image(profile) => {
                Some(profile)
            }
            ReapAuthorization::Record(_) | ReapAuthorization::Prefix => None,
        };
        self.inner
            .worker
            .delete_vm(ReapRequest {
                name: &name,
                authorization: &candidate.authorization,
                profile: profile_name.and_then(|profile| config.profiles.get(profile)),
                record: profile_name
                    .and_then(|profile| images.iter().find(|record| &record.profile == profile)),
                reserved: &reserved_image_names(&config),
            })
            .await?;
        tracing::info!(name = %candidate.name, authorization = ?candidate.authorization, age_seconds = candidate.age_seconds, "reaper deleted an unreferenced VM");
        Ok(true)
    }

    /// Drops a record once no image it names survives, no live profile still
    /// points at it, and no regeneration is mid-promotion on that profile. The
    /// configuration is re-read, because the sweep it follows may have run for
    /// minutes.
    async fn prune_image_records(&self) -> Vec<ProfileName> {
        let config = self.inner.config.current();
        let (Ok(images), Ok(machines)) = (
            self.inner.images.load_all().await,
            self.inner.worker.machines().await,
        ) else {
            return Vec::new();
        };
        let regenerating = self.regenerating_profiles().await;
        let mut pruned = Vec::new();
        for record in &images {
            let is_referenced = config
                .profiles
                .get(&record.profile)
                .and_then(|profile| profile.warm_template.as_ref())
                .is_some_and(|warm| warm == &record.warm_template);
            if is_referenced
                || is_present(record, &machines)
                || regenerating.contains(&record.profile)
            {
                continue;
            }
            if let Err(error) = self.inner.images.remove(&record.profile).await {
                tracing::warn!(profile = %record.profile, %error, "cannot remove an orphaned warm image record");
                continue;
            }
            tracing::info!(profile = %record.profile, warm_template = %record.warm_template, generation = record.generation, "reaper dropped a warm image record whose images are all gone");
            pruned.push(record.profile.clone());
        }
        pruned
    }

    async fn regenerating_profiles(&self) -> BTreeSet<ProfileName> {
        self.allocation_snapshot()
            .await
            .into_iter()
            .filter(|allocation| {
                !allocation.state.is_terminal() && allocation.mode == AllocationMode::Regenerate
            })
            .map(|allocation| allocation.request.profile)
            .collect()
    }

    pub(crate) async fn allocation_snapshot(&self) -> Vec<Allocation> {
        self.inner
            .entries
            .lock()
            .await
            .values()
            .map(|entry| entry.allocation.clone())
            .collect()
    }
}

/// An unset `runtime.tart_home` makes every age unknown, and the planner skips
/// what it cannot age, so the pass is inert rather than clean.
fn inert_reason(machines: &[HostMachine]) -> Option<SweepInertReason> {
    (!machines.is_empty() && machines.iter().all(|machine| machine.age_seconds.is_none()))
        .then_some(SweepInertReason::AgeUndeterminable)
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
