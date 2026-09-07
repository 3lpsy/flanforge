use std::time::Duration;

use flanforge_store::{Event, EventKind};

use flanforge_core::{HotDrainReason, HotGuest, LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS};

use super::super::AllocationManager;

impl AllocationManager {
    /// Destroys one pool machine and retires its record.
    ///
    /// **Why this never goes through `delete_vm`.** Every `ReapAuthorization`
    /// answers the same question — "does anything still reference this name?"
    /// — and a hot machine is referenced, by its own record, right up to the
    /// moment it is destroyed. Routing eviction through the reaper would mean
    /// teaching `protected_names` to un-protect a name it is protecting, which
    /// is the one invariant that keeps the sweep from eating the pool. So
    /// eviction lives here, `ensure_hot_evicted` takes a `&HotGuest` rather
    /// than a name, and destroying a pool machine without its record is
    /// unrepresentable. It also lets a backend do more than delete: on Tart the
    /// supervised `tart run` child has to be dropped, which `delete_vm` has no
    /// way to express.
    ///
    /// The record is removed only after the machine is gone. A record that
    /// outlives a failed deletion keeps the name protected and the slot
    /// charged, which is the safe direction to fail in: the alternative leaves
    /// a running machine nothing accounts for. It also stamps its
    /// `drain_reason`, which is what `ensure_hot_bounded` retries on — without
    /// that a record in a mid-operation state would be unreachable by every
    /// automatic path and hold its slot for the life of the daemon.
    pub(crate) async fn ensure_hot_evicted(&self, guest: &HotGuest, reason: HotDrainReason) {
        let budget = self.teardown_budget(Duration::from_secs(self.hot_eviction_seconds()));
        match self.inner.worker.ensure_hot_evicted(guest, budget).await {
            Ok(()) => {
                tracing::info!(
                    vm_name = %guest.vm_name,
                    profile = %guest.profile,
                    ?reason,
                    jobs_served = guest.jobs_served,
                    "hot guest evicted"
                );
            }
            Err(error) => {
                tracing::error!(vm_name = %guest.vm_name, %error, ?reason, "cannot evict a hot guest; its record is kept so the name stays protected and the slot stays charged");
                self.ensure_hot_eviction_pending(guest, reason).await;
                return;
            }
        }
        if let Err(error) = self.inner.hot.remove(&guest.vm_name).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "the evicted hot guest's record was not retired");
        }
        self.emit(
            Event::new(EventKind::HotEvicted)
                .with_vm_name(guest.vm_name.clone())
                .with_profile(guest.profile.clone())
                .with_payload(
                    &serde_json::json!({"reason": reason, "jobs_served": guest.jobs_served}),
                ),
        )
        .await;
    }

    /// Marks a record that has to go but cannot move to `Draining` yet: an
    /// eviction that failed, or a drain that landed on a mid-operation record.
    /// Only the reason is stamped, because the state machine has no edge out of
    /// a mid-operation state and inventing one would let a sweep destroy a
    /// machine whose retention is still in flight. The reason is what
    /// `ensure_hot_bounded` retries on and what `ensure_hot_gated` reads before
    /// it lets a machine join the pool.
    pub(super) async fn ensure_hot_eviction_pending(
        &self,
        guest: &HotGuest,
        reason: HotDrainReason,
    ) {
        if guest.drain_reason == Some(reason) {
            return;
        }
        let mut pending = guest.clone();
        pending.drain_reason = Some(reason);
        if let Err(error) = self.inner.hot.save(&pending).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "a failed hot eviction was not marked for retry");
        }
    }

    /// The reason a record was stamped with while an operation was still
    /// running over it, so the operation ends in an eviction rather than
    /// handing the machine back to the pool.
    pub(super) async fn hot_drain_pending(
        &self,
        vm_name: &flanforge_core::VmName,
    ) -> Option<HotDrainReason> {
        self.inner
            .hot
            .load(vm_name)
            .await
            .ok()
            .flatten()
            .and_then(|guest| guest.drain_reason)
    }

    /// Retires the record of a machine the host no longer reports.
    ///
    /// Deliberately not `ensure_hot_evicted`: there is nothing to destroy, so
    /// asking a backend to destroy it would turn "the machine is gone" into a
    /// failure and keep the record forever. This is the ordinary outcome of a
    /// Tart daemon restart, where every `tart run` child died with its parent.
    pub(crate) async fn ensure_hot_forgotten(&self, guest: &HotGuest, reason: HotDrainReason) {
        tracing::info!(
            vm_name = %guest.vm_name,
            profile = %guest.profile,
            ?reason,
            "retiring the record of a hot guest the host no longer reports"
        );
        if let Err(error) = self.inner.hot.remove(&guest.vm_name).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "a gone hot guest's record was not retired");
        }
        self.emit(
            Event::new(EventKind::HotEvicted)
                .with_vm_name(guest.vm_name.clone())
                .with_profile(guest.profile.clone())
                .with_payload(&serde_json::json!({"reason": reason, "machine_present": false})),
        )
        .await;
    }

    /// The widest configured teardown allowance, because destroying a pool
    /// machine is the same work destroying an allocation's guest is and no
    /// profile is more entitled to it than another. A running shutdown caps it.
    fn hot_eviction_seconds(&self) -> u64 {
        self.inner
            .config
            .current()
            .profiles
            .values()
            .map(|profile| profile.cleanup_timeout_seconds)
            .max()
            .unwrap_or(LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS)
            .max(LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS)
    }
}
