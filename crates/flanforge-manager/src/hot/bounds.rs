use std::collections::BTreeSet;

use flanforge_core::{Config, HotDrainReason, HotGuest, HotState, unix_time};

use super::{
    super::AllocationManager,
    select::{HotPolicy, drain_reason, surplus_idle},
};

impl AllocationManager {
    /// Applies every lifecycle bound to the whole pool and acts on what it
    /// finds. Run after a release and on every deleting sweep, so an idle
    /// machine gives its slot back without anyone asking.
    pub(crate) async fn ensure_hot_bounded(&self) {
        let config = self.inner.config.current();
        let guests = match self.inner.hot.load_all().await {
            Ok(guests) => guests,
            Err(error) => {
                tracing::error!(%error, "hot guest records are unreadable; bounds were not applied");
                return;
            }
        };
        let now = unix_time();
        if config.runtime.max_hot_vms == 0 {
            // The documented global kill switch. It has to drain rather than
            // freeze: a pool left standing keeps its capacity charge, and
            // `ensure_hot_yielded` loops `max_hot_vms` times, so nothing else
            // would ever free those slots.
            self.ensure_hot_pool_drained(&guests).await;
            return;
        }
        for guest in &guests {
            if guest.state.is_terminal() {
                continue;
            }
            let Some(hot) = config
                .profiles
                .get(&guest.profile)
                .and_then(|profile| profile.hot)
                .filter(|hot| hot.enabled)
            else {
                // Nothing live configures this machine any more.
                self.ensure_hot_drained(guest, HotDrainReason::ConfigReloaded)
                    .await;
                continue;
            };
            let fingerprint = match config.profiles.get(&guest.profile) {
                Some(profile) => self.inner.worker.base_fingerprint(&profile.template).await,
                None => None,
            };
            let policy = HotPolicy {
                hot: &hot,
                fingerprint: fingerprint.as_ref(),
                // The generation the profile promotes *now*, resolved live the
                // way the fingerprint above is. Reading it off the record would
                // compare the machine with itself and never drain.
                warm_generation: self.promoted_generation(&guest.profile).await,
                now,
            };
            if let Some(reason) = drain_reason(guest, policy) {
                self.ensure_hot_drained(guest, reason).await;
            }
        }
        self.ensure_hot_surplus_retired(&guests, &config).await;
        self.ensure_hot_drains_finished(&guests).await;
        self.ensure_hot_evictions_retried(&guests).await;
    }

    /// Drains every machine in the pool, whatever state it is in. Only
    /// `runtime.max_hot_vms = 0` reaches this: the operator asked for no pool
    /// at all, and that answer does not wait for a bound to expire.
    async fn ensure_hot_pool_drained(&self, guests: &[HotGuest]) {
        for guest in guests.iter().filter(|guest| !guest.state.is_terminal()) {
            self.ensure_hot_drained(guest, HotDrainReason::ConfigReloaded)
                .await;
        }
        self.ensure_hot_drains_finished(guests).await;
        self.ensure_hot_evictions_retried(guests).await;
    }

    /// Retires idle machines a lane holds beyond `max_idle`. This is where the
    /// per-lane ceiling binds, because a machine joins a lane at release.
    async fn ensure_hot_surplus_retired(&self, guests: &[HotGuest], config: &Config) {
        let lanes = guests
            .iter()
            .filter(|guest| guest.state == HotState::Idle)
            .map(|guest| (guest.profile.clone(), guest.lane))
            .collect::<BTreeSet<_>>();
        for (name, lane) in lanes {
            let Some(hot) = config
                .profiles
                .get(&name)
                .and_then(|profile| profile.hot)
                .filter(|hot| hot.enabled)
            else {
                continue;
            };
            for guest in surplus_idle(guests, &name, lane, &hot) {
                tracing::info!(vm_name = %guest.vm_name, profile = %name, ?lane, "retiring an idle hot guest above max_idle");
                self.ensure_hot_drained(guest, HotDrainReason::MaxIdle)
                    .await;
            }
        }
    }

    /// A draining machine nobody is using is a machine to destroy. This is the
    /// only place a drain becomes an eviction, so `Draining` always means
    /// "waiting for a claim to end" and never "forgotten".
    async fn ensure_hot_drains_finished(&self, guests: &[HotGuest]) {
        let claimed = self.hot_claimed_names().await;
        for guest in guests
            .iter()
            .filter(|guest| guest.state == HotState::Draining)
            .filter(|guest| !claimed.contains(guest.vm_name.as_str()))
        {
            self.ensure_hot_evicted(
                guest,
                guest
                    .drain_reason
                    .unwrap_or(HotDrainReason::OperatorRequest),
            )
            .await;
        }
    }

    /// Retries every eviction that failed. A record carries a `drain_reason`
    /// only once something decided the machine had to go, so this cannot race
    /// a retention still in flight — and without it a record whose state has
    /// no edge to `Draining` would hold its slot until the daemon restarts.
    async fn ensure_hot_evictions_retried(&self, guests: &[HotGuest]) {
        let claimed = self.hot_claimed_names().await;
        for guest in guests
            .iter()
            .filter(|guest| guest.state != HotState::Draining)
            .filter(|guest| guest.claimed_by.is_none())
            .filter(|guest| !claimed.contains(guest.vm_name.as_str()))
            .filter(|guest| guest.drain_reason.is_some())
        {
            let reason = guest
                .drain_reason
                .unwrap_or(HotDrainReason::OperatorRequest);
            tracing::warn!(vm_name = %guest.vm_name, ?reason, state = ?guest.state, "retrying a hot eviction that did not complete");
            self.ensure_hot_evicted(guest, reason).await;
        }
    }

    async fn hot_claimed_names(&self) -> BTreeSet<String> {
        let entries = self.inner.entries.lock().await;
        super::claim::live_hot_names(&entries)
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Stops a machine taking new claims. A claimed machine keeps its job and
    /// is destroyed when the claim ends; an unclaimed one is destroyed now.
    pub(crate) async fn ensure_hot_drained(&self, guest: &HotGuest, reason: HotDrainReason) {
        let mut draining = guest.clone();
        if draining.state == HotState::Draining {
            return;
        }
        if draining.ensure_state(HotState::Draining).is_err() {
            // `Provisioning` and `Recycling` have no edge to `Draining`: both
            // are mid-operation. Stamping the reason is what that operation
            // reads before it joins the pool and what the next sweep retries
            // on, so a mid-operation record is never unreachable.
            tracing::info!(vm_name = %guest.vm_name, state = ?guest.state, ?reason, "a mid-operation hot guest is marked to go when its operation ends");
            self.ensure_hot_eviction_pending(guest, reason).await;
            return;
        }
        draining.drain_reason = Some(reason);
        if let Err(error) = self.inner.hot.save(&draining).await {
            tracing::error!(vm_name = %guest.vm_name, %error, "cannot record a hot drain");
            return;
        }
        tracing::info!(vm_name = %guest.vm_name, profile = %guest.profile, ?reason, "hot guest is draining");
        if draining.claimed_by.is_none() {
            self.ensure_hot_evicted(&draining, reason).await;
        }
    }
}
