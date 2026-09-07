use std::time::Duration;

use flanforge_core::{Allocation, HotDrainReason, HotGuest, HotLane, HotState, Profile};

use super::{
    super::{AllocationManager, HotReset, HotRetainRequest},
    claim::retained_record,
};

impl AllocationManager {
    /// Hands a machine to the pool once its job has ended, or lets teardown
    /// destroy it.
    ///
    /// Two ways in, one way out. An allocation that reused a machine releases
    /// it; one that was flagged for retention hands over the guest it cloned.
    /// Both then run the recycle gate, and both fail the same way: anything
    /// but a clean pass destroys the machine and the next request clones.
    /// Recycling never fails a job — the job is already over.
    ///
    /// The backend is told the outcome by whether it holds the machine, which
    /// is what its own `cleanup` reads; nothing here has to return it.
    /// Idempotent, because `ensure_terminal` is: a pass that fails at its
    /// final transition leaves the allocation non-terminal, and a later cancel
    /// re-enters. Both arms below decide from the durable record, so a second
    /// pass finds the release already made and does nothing.
    pub(crate) async fn ensure_hot_released(&self, allocation: &Allocation, profile: &Profile) {
        if let Some(vm_name) = allocation.origin.hot_vm_name() {
            self.ensure_hot_recycled(allocation, vm_name.clone(), profile)
                .await;
            return;
        }
        if let Some(lane) = allocation.hot_lane {
            self.ensure_hot_retained(allocation, profile, lane).await;
        }
    }

    /// Releases the machine this allocation reused back into the pool.
    async fn ensure_hot_recycled(
        &self,
        allocation: &Allocation,
        vm_name: flanforge_core::VmName,
        profile: &Profile,
    ) {
        let guest = match self.inner.hot.load(&vm_name).await {
            Ok(Some(guest)) => guest,
            Ok(None) => {
                tracing::warn!(%vm_name, "the hot record this allocation claimed is gone");
                return;
            }
            Err(error) => {
                tracing::error!(%vm_name, %error, "cannot read the hot record to release it");
                return;
            }
        };
        // Only this allocation's own claim is this allocation's to release. A
        // record already back in the pool means an earlier pass released it,
        // and evicting on that would destroy a machine the pool legitimately
        // holds — possibly one another allocation has since claimed.
        if guest.claimed_by != Some(allocation.id) {
            tracing::debug!(%vm_name, allocation_id = %allocation.id, state = ?guest.state, "the hot claim was already released");
            return;
        }
        let mut guest = guest;
        if guest.ensure_released().is_err() {
            self.ensure_hot_evicted(&guest, HotDrainReason::ResetFailed)
                .await;
            return;
        }
        if let Err(error) = self.inner.hot.save(&guest).await {
            tracing::error!(%vm_name, %error, "cannot record the hot release; evicting the machine");
            self.ensure_hot_evicted(&guest, HotDrainReason::ResetFailed)
                .await;
            return;
        }
        self.ensure_hot_gated(guest, profile, None).await;
    }

    /// Takes over the guest a hot-flagged allocation cloned. The backend keeps
    /// the machine alive past the worker that built it, which on Tart is the
    /// whole difference: the `tart run` child has to leave the worker's scope.
    async fn ensure_hot_retained(&self, allocation: &Allocation, profile: &Profile, lane: HotLane) {
        if !allocation.vm_created {
            return;
        }
        let Some(hot) = profile.hot.filter(|hot| hot.enabled) else {
            return;
        };
        // A record already naming this machine is an earlier pass's retention;
        // overwriting it would reset the machine's history and re-run the gate
        // over a guest the pool is already serving from.
        if matches!(self.inner.hot.load(&allocation.vm_name).await, Ok(Some(_))) {
            tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "the guest was already retained");
            return;
        }
        let Some(guest) = retained_record(allocation, lane) else {
            tracing::warn!(allocation_id = %allocation.id, "a hot allocation recorded no size or source, so its guest cannot be retained");
            return;
        };
        // The record is written before the machine is anyone else's, so a
        // machine the pool holds is never a machine with no record.
        if let Err(error) = self.inner.hot.save(&guest).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "cannot record a retained hot guest; it is torn down as usual");
            return;
        }
        let reset = HotReset {
            simulator_reset: hot.simulator_reset,
            budget: self.teardown_budget(Duration::from_secs(hot.reset_timeout_seconds)),
        };
        if let Err(error) = self
            .inner
            .worker
            .ensure_hot_retained(HotRetainRequest {
                guest: &guest,
                allocation,
                profile,
                reset,
            })
            .await
        {
            tracing::warn!(vm_name = %guest.vm_name, %error, "the backend could not take the guest into the pool; it is torn down as usual");
            if let Err(error) = self.inner.hot.remove(&guest.vm_name).await {
                tracing::error!(vm_name = %guest.vm_name, %error, "a withdrawn retention left its record behind");
                // `Provisioning` has no edge to `Draining`, so without a reason
                // stamped on it this record would hold a slot and a size until
                // the daemon restarts.
                self.ensure_hot_eviction_pending(&guest, HotDrainReason::ResetFailed)
                    .await;
            }
            return;
        }
        self.ensure_hot_gated(guest, profile, Some(reset)).await;
    }

    /// The one gate both paths run through: reset, then join the pool or be
    /// destroyed. `already_reset` carries the retention's own pass so the gate
    /// is not run twice over one machine.
    async fn ensure_hot_gated(
        &self,
        mut guest: HotGuest,
        profile: &Profile,
        already_reset: Option<HotReset>,
    ) {
        let Some(hot) = profile.hot.filter(|hot| hot.enabled) else {
            // The profile stopped naming hot while this machine was claimed.
            self.ensure_hot_evicted(&guest, HotDrainReason::ConfigReloaded)
                .await;
            return;
        };
        if already_reset.is_none() {
            let reset = HotReset {
                simulator_reset: hot.simulator_reset,
                budget: self.teardown_budget(Duration::from_secs(hot.reset_timeout_seconds)),
            };
            if let Err(error) = self.inner.worker.ensure_hot_reset(&guest, reset).await {
                tracing::warn!(vm_name = %guest.vm_name, %error, "the recycle gate did not pass; the machine is evicted and the next request clones");
                self.ensure_hot_evicted(&guest, HotDrainReason::ResetFailed)
                    .await;
                return;
            }
        }
        // A drain stamped while the gate ran — `hot: "evict"`, a reload, the
        // global kill switch — is the answer, not this pass's clean verdict.
        // `Recycling` has no edge to `Draining`, so this read is the only place
        // that request can be honoured before the machine serves another job.
        if let Some(reason) = self.hot_drain_pending(&guest.vm_name).await {
            tracing::info!(vm_name = %guest.vm_name, ?reason, "a hot guest was drained while its recycle gate ran; it does not rejoin the pool");
            self.ensure_hot_evicted(&guest, reason).await;
            return;
        }
        if guest.ensure_state(HotState::Idle).is_err() || self.inner.hot.save(&guest).await.is_err()
        {
            tracing::error!(vm_name = %guest.vm_name, "a reset machine could not join the pool; evicting it rather than leaving it unclaimable");
            self.ensure_hot_evicted(&guest, HotDrainReason::ResetFailed)
                .await;
            return;
        }
        tracing::info!(
            vm_name = %guest.vm_name,
            profile = %guest.profile,
            lane = ?guest.lane,
            jobs_served = guest.jobs_served,
            "hot guest is in the pool"
        );
        self.emit(
            flanforge_store::Event::new(flanforge_store::EventKind::HotRecycled)
                .with_vm_name(guest.vm_name.clone())
                .with_profile(guest.profile.clone())
                .with_payload(
                    &serde_json::json!({"lane": guest.lane, "jobs_served": guest.jobs_served}),
                ),
        )
        .await;
        // The machine it just joined may now be one too many for its lane.
        self.ensure_hot_bounded().await;
    }
}
