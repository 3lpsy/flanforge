use flanforge_core::{Config, GuestSize, HotDrainReason, HotGuest, HotState};

use super::super::{
    AllocationManager,
    admission::{active_allocations, hot_names},
};

impl AllocationManager {
    /// Frees a slot an idle retained machine is holding, when this request
    /// cannot otherwise be admitted.
    ///
    /// A retained machine is a cache, and a cache yields under pressure.
    /// Without this, `max_running_vms = 1` with one machine retained for four
    /// hours refuses every allocation for four hours: nothing but the reaper's
    /// one-hour age floor would collect it, and the age floor does not apply
    /// to a name a hot record protects.
    ///
    /// Only an *idle* machine yields. A claimed one is running somebody's job,
    /// and evicting it would fail that job to admit this one.
    ///
    /// Best effort by design: it evicts while capacity refuses and machines
    /// remain, and if admission still refuses afterwards the caller gets an
    /// honest busy answer.
    pub(crate) async fn ensure_hot_yielded(&self, config: &Config, requested: GuestSize) {
        // Bounded by what the pool actually holds rather than by
        // `max_hot_vms`: that cap can be lowered — to zero, the kill switch —
        // while machines retained under the old one still hold their slots, and
        // those are exactly the machines this has to be able to free.
        let held = self.inner.hot.load_all().await.map_or(0, |hot| hot.len());
        for _ in 0..held {
            let Some(guest) = self.yieldable_hot(config, requested).await else {
                return;
            };
            tracing::info!(
                vm_name = %guest.vm_name,
                profile = %guest.profile,
                "evicting an idle hot guest so an allocation the host has no other room for can be admitted"
            );
            self.ensure_hot_evicted(&guest, HotDrainReason::CapacityPressure)
                .await;
        }
    }

    /// The stalest idle machine, when and only when admission would refuse
    /// without freeing one. Folding capacity here rather than trusting a
    /// counter means the answer is the same one `create` will get.
    async fn yieldable_hot(&self, config: &Config, requested: GuestSize) -> Option<HotGuest> {
        let machines = self.host_machines().await.ok()?;
        let hot = self.inner.hot.load_all().await.ok()?;
        let entries = self.inner.entries.lock().await;
        if Self::ensure_capacity(&entries, config, requested, &machines, &hot).is_ok() {
            return None;
        }
        let claimed = super::claim::live_hot_names(&entries);
        let names = hot_names(&hot, &machines);
        let active = active_allocations(&entries);
        // A machine an active allocation still names is being torn down, not
        // idling; charging it as free would evict a guest mid-teardown.
        let busy = active
            .iter()
            .map(|allocation| allocation.vm_name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        hot.iter()
            .filter(|guest| guest.state == HotState::Idle)
            .filter(|guest| names.contains(guest.vm_name.as_str()))
            .filter(|guest| !claimed.contains(guest.vm_name.as_str()))
            .filter(|guest| !busy.contains(guest.vm_name.as_str()))
            .min_by_key(|guest| (guest.updated_at_unix, guest.vm_name.as_str().to_owned()))
            .cloned()
    }
}
