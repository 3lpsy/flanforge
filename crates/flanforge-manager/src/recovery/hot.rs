use std::time::Duration;

use flanforge_core::{HotDrainReason, HotGuest, HotState, Profile};

use super::super::{AllocationManager, HotReset, ManagerError};

impl AllocationManager {
    /// Reconciles every durable hot record against the host listing.
    ///
    /// Runs after the interrupted-allocation pass, so a machine an allocation
    /// was holding has already been released and recycled through the normal
    /// path rather than reconciled twice.
    ///
    /// Adoption requires proof, and the recycle gate is the proof: a machine
    /// the host still reports is re-verified before it may serve another job,
    /// and anything the gate does not pass is evicted. A record whose machine
    /// is absent is evicted too — that is what a Tart daemon restart does to
    /// every guest it owned, because `tart run` is a child of `flanforged`.
    ///
    /// # Errors
    ///
    /// Returns an error only for store-wide conditions; a record that cannot
    /// be reconciled is logged and counted so startup still completes.
    pub(super) async fn ensure_hot_consistent(&self) -> Result<(), ManagerError> {
        let guests = self.inner.hot.load_all().await?;
        if guests.is_empty() {
            return Ok(());
        }
        let config = self.inner.config.current();
        let machines = self.inner.worker.machines().await?;
        let mut adopted = 0_usize;
        let mut retired = 0_usize;
        for guest in guests {
            if guest.state.is_terminal() {
                if let Err(error) = self.inner.hot.remove(&guest.vm_name).await {
                    tracing::error!(vm_name = %guest.vm_name, %error, "a terminal hot record was not dropped");
                }
                continue;
            }
            let is_present = machines
                .iter()
                .any(|machine| machine.name == guest.vm_name.as_str());
            let profile = config.profiles.get(&guest.profile);
            if self.is_hot_adoptable(&guest, is_present, profile).await {
                adopted += 1;
            } else {
                retired += 1;
            }
        }
        tracing::info!(adopted, retired, "hot guest records reconciled");
        Ok(())
    }

    /// Verifies one surviving machine and returns it to the pool, or evicts it.
    /// Every path that is not a clean gate pass ends in an eviction, so the
    /// pool never inherits a machine nobody proved clean.
    async fn is_hot_adoptable(
        &self,
        guest: &HotGuest,
        is_present: bool,
        profile: Option<&Profile>,
    ) -> bool {
        if !is_present {
            tracing::info!(
                vm_name = %guest.vm_name,
                profile = %guest.profile,
                reason = ?HotDrainReason::MachineGone,
                "hot guest did not survive the restart"
            );
            self.ensure_hot_forgotten(guest, HotDrainReason::MachineGone)
                .await;
            return false;
        }
        // A machine an operator drained does not come back claimable: the
        // reason outlives the restart, and `claimable` does not read it.
        if let Some(reason) = guest.drain_reason {
            tracing::info!(vm_name = %guest.vm_name, ?reason, "a hot guest was already draining before the restart");
            self.ensure_hot_evicted(guest, reason).await;
            return false;
        }
        let Some(hot) = profile
            .and_then(|profile| profile.hot)
            .filter(|hot| hot.enabled)
        else {
            tracing::warn!(vm_name = %guest.vm_name, "no live profile still configures this hot guest");
            self.ensure_hot_evicted(guest, HotDrainReason::ConfigReloaded)
                .await;
            return false;
        };
        let reset = HotReset {
            simulator_reset: hot.simulator_reset,
            budget: self.teardown_budget(Duration::from_secs(hot.reset_timeout_seconds)),
        };
        if let Err(error) = self.inner.worker.ensure_hot_reset(guest, reset).await {
            tracing::warn!(vm_name = %guest.vm_name, %error, reason = ?HotDrainReason::Unverifiable, "a surviving hot guest could not be verified clean");
            self.ensure_hot_evicted(guest, HotDrainReason::Unverifiable)
                .await;
            return false;
        }
        let mut adopted = guest.clone();
        // Whatever state the restart interrupted, a verified machine is idle:
        // no allocation survives a restart still holding it. The claim is given
        // up through the record rather than assigned past, so a state with no
        // edge back to the pool evicts instead of being written over.
        if adopted.state == HotState::Claimed {
            let _ = adopted.ensure_released();
        }
        if adopted.ensure_state(HotState::Idle).is_err() {
            tracing::warn!(vm_name = %guest.vm_name, state = ?guest.state, "a surviving hot guest has no edge back to the pool; evicting it");
            self.ensure_hot_evicted(guest, HotDrainReason::Unverifiable)
                .await;
            return false;
        }
        if let Err(error) = self.inner.hot.save(&adopted).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "a verified hot guest could not be recorded; evicting it");
            self.ensure_hot_evicted(guest, HotDrainReason::Unverifiable)
                .await;
            return false;
        }
        tracing::info!(
            vm_name = %guest.vm_name,
            profile = %guest.profile,
            jobs_served = guest.jobs_served,
            "hot guest survived the restart and passed the recycle gate"
        );
        true
    }
}
