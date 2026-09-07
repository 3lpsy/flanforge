use std::{collections::BTreeSet, time::Duration};

use flanforge_core::{
    Allocation, AllocationMode, Config, LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS, Profile, ProfileName,
    VmName, unix_time,
};
use serde::{Deserialize, Serialize};

use super::{
    super::{AllocationManager, HostMachine, ManagerError, ReapRequest, RetiredImage},
    plan::{
        ReapAuthorization, ReapCandidate, ReapInputs, plan_sweep, reserved_image_names,
        unaged_candidates,
    },
};

/// Why a pass could plan nothing, so an empty report is not read as a clean
/// host when the daemon simply cannot tell.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepInertReason {
    /// No machine age is determinable. On Tart that is a library location
    /// that cannot be derived or read at all; on libvirt it is no domain
    /// carrying an ownership manifest.
    AgeUndeterminable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SweepReport {
    pub planned: Vec<ReapCandidate>,
    pub deleted: Vec<String>,
    pub skipped: Vec<String>,
    /// Prefix-owned names dropped for want of a determinable age. The sweep
    /// cannot apply its age gate to them, so it never collects them.
    #[serde(default)]
    pub unaged: Vec<String>,
    /// Warm image records dropped because nothing they name survives.
    #[serde(default)]
    pub pruned_records: Vec<ProfileName>,
    /// Warm generations the backend considered for retirement. Populated on a
    /// dry run too, with `is_deleted` false.
    #[serde(default)]
    pub retired_images: Vec<RetiredImage>,
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
        // Lifecycle bounds first, and only on a deleting pass: a machine past
        // max_lifetime or idle_ttl must give its slot back without an operator
        // asking, and a dry run reports rather than acts.
        if !is_dry_run {
            self.ensure_hot_bounded().await;
        }
        let config = self.inner.config.current();
        let images = self.inner.images.load_all().await?;
        let hot = self.inner.hot.load_all().await?;
        let machines = self.inner.worker.machines().await?;
        let inert_reason = inert_reason(&machines);
        let allocations = self.allocation_snapshot().await;
        let inputs = ReapInputs {
            config: &config,
            machines: &machines,
            allocations: &allocations,
            images: &images,
            hot: &hot,
        };
        let planned = plan_sweep(inputs);
        let unaged = unaged_candidates(inputs);
        if !unaged.is_empty() {
            tracing::warn!(
                names = ?unaged,
                "prefix-owned VMs have no determinable age, so the sweep can never collect them"
            );
        }
        tracing::info!(
            candidates = planned.len(),
            unaged = unaged.len(),
            hot_protected = hot
                .iter()
                .filter(|guest| guest.state.is_holding_machine())
                .count(),
            is_dry_run,
            ?inert_reason,
            "reaper sweep planned"
        );

        let mut deleted = Vec::new();
        let mut skipped = Vec::new();
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
        }
        // A dry run must see the same candidates a real run would, so the
        // prune is planned either way and only applied when asked.
        let plan = self.plan_pruned_records(&images, &machines).await;
        let pruned_records = if is_dry_run {
            Vec::new()
        } else {
            self.apply_pruned_records(&plan.prunable).await
        };
        let retired_images = self.sweep_warm_images(is_dry_run, &plan.claimed).await;

        let report = SweepReport {
            planned,
            deleted,
            skipped,
            unaged,
            pruned_records,
            retired_images,
            inert_reason,
            finished_at_unix: unix_time(),
        };
        tracing::info!(
            deleted = report.deleted.len(),
            skipped = report.skipped.len(),
            unaged = report.unaged.len(),
            pruned_records = report.pruned_records.len(),
            retired_images = report.retired_images.len(),
            "reaper sweep finished"
        );
        if !is_dry_run {
            *self.inner.last_sweep.lock().await = Some(report.clone());
            for name in &report.deleted {
                self.emit(
                    flanforge_store::Event::new(flanforge_store::EventKind::ReaperDeleted)
                        .with_payload(&serde_json::json!({"name": name})),
                )
                .await;
            }
            self.emit(
                flanforge_store::Event::new(flanforge_store::EventKind::ReaperSweep).with_payload(
                    &serde_json::json!({
                        "planned": report.planned.len(),
                        "deleted": report.deleted.len(),
                        "skipped": report.skipped.len(),
                        "unaged": report.unaged.len(),
                        "pruned_records": report.pruned_records.len(),
                        "retired_images": report.retired_images.len(),
                    }),
                ),
            )
            .await;
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
        let Ok(name) = VmName::new(candidate.name.clone()) else {
            return Ok(false);
        };
        let config = self.inner.config.current();
        let images = self.inner.images.load_all().await?;
        // Re-read: a hot guest provisioned since the plan must claim its name
        // here, or the sweep deletes a machine that was just created.
        let hot = self.inner.hot.load_all().await?;
        let allocations = self.allocation_snapshot().await;
        let fresh = plan_sweep(ReapInputs {
            config: &config,
            machines: &self.inner.worker.machines().await?,
            allocations: &allocations,
            images: &images,
            hot: &hot,
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
        let reaped = match &candidate.authorization {
            ReapAuthorization::Record(id) => {
                allocations.iter().find(|allocation| allocation.id == *id)
            }
            _ => None,
        };
        let profile = match reaped {
            Some(allocation) => config.profiles.get(&allocation.request.profile),
            None => profile_name.and_then(|profile| config.profiles.get(profile)),
        };
        self.inner
            .worker
            .delete_vm(ReapRequest {
                name: &name,
                authorization: &candidate.authorization,
                profile,
                record: profile_name
                    .and_then(|profile| images.iter().find(|record| &record.profile == profile)),
                allocation: reaped,
                reserved: &reserved_image_names(&config),
                budget: self.teardown_budget(cleanup_allowance(&config, profile)),
            })
            .await?;
        tracing::info!(name = %candidate.name, authorization = ?candidate.authorization, age_seconds = candidate.age_seconds, "reaper deleted an unreferenced VM");
        Ok(true)
    }

    pub(super) async fn regenerating_profiles(&self) -> BTreeSet<ProfileName> {
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

/// The teardown allowance a deletion gets. A guest one of the daemon's
/// profiles still names is torn down on that profile's allowance; an orphan
/// none names gets the widest configured one, because the shared minimum
/// cannot cover a stop, an undefine, and two volume deletes. A running
/// shutdown caps whatever this returns.
fn cleanup_allowance(config: &Config, profile: Option<&Profile>) -> Duration {
    let seconds = profile
        .map(|profile| profile.cleanup_timeout_seconds)
        .or_else(|| {
            config
                .profiles
                .values()
                .map(|profile| profile.cleanup_timeout_seconds)
                .max()
        })
        .unwrap_or(LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS)
        .max(LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS);
    Duration::from_secs(seconds)
}

/// A backend that can age nothing at all leaves the planner nothing to work
/// with, so the pass is inert rather than clean.
fn inert_reason(machines: &[HostMachine]) -> Option<SweepInertReason> {
    (!machines.is_empty() && machines.iter().all(|machine| machine.age_seconds.is_none()))
        .then_some(SweepInertReason::AgeUndeterminable)
}
