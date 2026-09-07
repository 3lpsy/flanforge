use std::collections::BTreeSet;

use flanforge_core::{Config, HotDrainReason, ProfileName, VmName, is_hot_draining_change};
use tokio_util::sync::CancellationToken;

use super::super::{AllocationManager, ManagerError};

impl AllocationManager {
    /// Watches the running configuration and drains the pool on a change that
    /// invalidates a machine already built. A reload is not a sweep trigger, so
    /// this cannot wait for the next one: `reap_interval_hours` may be a day.
    pub fn spawn_hot_reload_watch(&self, shutdown: CancellationToken) {
        let manager = self.clone();
        let mut receiver = manager.inner.config.subscribe();
        tokio::spawn(async move {
            let mut previous = manager.inner.config.current();
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    changed = receiver.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
                let next = manager.inner.config.current();
                manager
                    .ensure_hot_reloaded(&drained_profiles(&previous, &next))
                    .await;
                // Then every other live bound against the new configuration,
                // which is what makes `runtime.max_hot_vms = 0` drain the pool
                // when it is set rather than at the next release.
                manager.ensure_hot_bounded().await;
                previous = next;
            }
        });
    }

    /// Drains every machine of every profile a reload invalidated. A running
    /// machine cannot be resized under a claim, and a machine sized by a
    /// superseded profile is not the machine the operator configured.
    pub(crate) async fn ensure_hot_reloaded(&self, changed: &BTreeSet<ProfileName>) {
        if changed.is_empty() {
            return;
        }
        let guests = match self.inner.hot.load_all().await {
            Ok(guests) => guests,
            Err(error) => {
                tracing::error!(%error, "hot guest records are unreadable; a reload drained nothing");
                return;
            }
        };
        for guest in guests
            .iter()
            .filter(|guest| guest.state.is_holding_machine())
            .filter(|guest| changed.contains(&guest.profile))
        {
            self.ensure_hot_drained(guest, HotDrainReason::ConfigReloaded)
                .await;
        }
    }

    /// Tears down every machine this profile has retained, because a run
    /// asked to. The workflow is the authority on when a hot session ends and
    /// it says so explicitly; a run that simply does not mention hot leaves
    /// the pool alone.
    ///
    /// Profile-scoped and covering both lanes: `hot: "evict"` reads
    /// profile-wide, and a machine in the other lane is no more wanted than
    /// one in this one. A machine still serving a job drains and goes when
    /// that claim ends; only an unclaimed one goes now.
    pub(crate) async fn ensure_hot_evicted_for(
        &self,
        profile: &ProfileName,
        reason: HotDrainReason,
    ) {
        let guests = match self.inner.hot.load_all().await {
            Ok(guests) => guests,
            Err(error) => {
                tracing::error!(%error, "hot guest records are unreadable; nothing was evicted");
                return;
            }
        };
        for guest in guests
            .iter()
            .filter(|guest| guest.state.is_holding_machine())
            .filter(|guest| &guest.profile == profile)
        {
            tracing::info!(vm_name = %guest.vm_name, %profile, ?reason, "ending this profile's hot session");
            self.ensure_hot_drained(guest, reason).await;
        }
    }

    /// Drains one machine an operator named, waiting for its claim to end.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when no record names the machine.
    pub async fn hot_drain(&self, vm_name: &VmName) -> Result<(), ManagerError> {
        let guest = self
            .inner
            .hot
            .load(vm_name)
            .await?
            .ok_or_else(|| ManagerError::UnknownHotGuest(vm_name.to_string()))?;
        self.ensure_hot_drained(&guest, HotDrainReason::OperatorRequest)
            .await;
        Ok(())
    }

    /// Destroys one machine an operator named, claim or no claim.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when no record names the machine.
    pub async fn hot_evict(&self, vm_name: &VmName) -> Result<(), ManagerError> {
        let guest = self
            .inner
            .hot
            .load(vm_name)
            .await?
            .ok_or_else(|| ManagerError::UnknownHotGuest(vm_name.to_string()))?;
        self.ensure_hot_evicted(&guest, HotDrainReason::OperatorRequest)
            .await;
        Ok(())
    }
}

/// Profiles whose machines a reload invalidated: one that changed in a way a
/// running guest cannot absorb, and one that is simply gone. `Config` is the
/// authority on the first, through a predicate with its own tests, so the
/// field list lives in one place rather than at this call site.
fn drained_profiles(previous: &Config, next: &Config) -> BTreeSet<ProfileName> {
    previous
        .profiles
        .iter()
        .filter(|(name, profile)| {
            next.profiles
                .get(*name)
                .is_none_or(|replacement| is_hot_draining_change(profile, replacement))
        })
        .map(|(name, _)| name.clone())
        .collect()
}
