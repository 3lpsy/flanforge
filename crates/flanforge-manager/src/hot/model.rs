use flanforge_core::{
    AllocationId, BaseFingerprint, Config, HotConfig, HotDrainReason, HotLane, HotState, Profile,
    ProfileName, VmName, unix_time,
};
use flanforge_wire::RuntimeCapability;
use serde::{Deserialize, Serialize};

use super::{super::AllocationManager, select::HotPolicy};

/// Everything a claim decision needs, read before the entries lock so the
/// decision under it is pure. Assembled once per `create`.
#[derive(Clone, Debug)]
pub(crate) struct HotContext {
    pub(crate) hot: HotConfig,
    pub(crate) name: ProfileName,
    pub(crate) lane: HotLane,
    pub(crate) fingerprint: Option<BaseFingerprint>,
    pub(crate) warm_generation: Option<u64>,
    pub(crate) max_hot_vms: u8,
    /// This run's own lifetime for the machine it claims or retains, already
    /// checked against the profile ceiling. `None` takes the ceiling.
    pub(crate) age_seconds: Option<u64>,
}

impl HotContext {
    pub(crate) fn policy(&self) -> HotPolicy<'_> {
        HotPolicy {
            hot: &self.hot,
            fingerprint: self.fingerprint.as_ref(),
            warm_generation: self.warm_generation,
            now: unix_time(),
        }
    }
}

/// One hot record as an operator sees it: a projection of the durable record,
/// not a second place state is kept.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HotGuestStatus {
    pub vm_name: VmName,
    pub profile: ProfileName,
    pub lane: HotLane,
    pub state: HotState,
    pub age_seconds: u64,
    /// Seconds since the record last moved, which is how long an `Idle`
    /// machine has been giving nothing back.
    pub idle_seconds: u64,
    pub jobs_served: u32,
    pub claimed_by: Option<AllocationId>,
    pub drain_reason: Option<HotDrainReason>,
    /// False when the host does not report the machine as running and owned.
    pub is_machine_present: bool,
}

impl AllocationManager {
    /// The live configuration's hot policy for this profile and lane, or
    /// `None` when nothing about it admits a hot claim.
    ///
    /// This is where the capability gate really lives. Configuration only
    /// warns, so without this re-check lifting the config gate would enable
    /// hot on a backend that cannot hold a machine open — the same reason
    /// `warm_source` re-checks `WarmImages` rather than trusting the load.
    pub(crate) async fn hot_context(
        &self,
        config: &Config,
        name: &ProfileName,
        profile: &Profile,
        lane: HotLane,
        age_seconds: Option<u64>,
    ) -> Option<HotContext> {
        let hot = profile.hot.filter(|hot| hot.enabled)?;
        if config.runtime.max_hot_vms == 0 || !hot.lanes.is_lane_admitted(lane) {
            return None;
        }
        if !self
            .inner
            .worker
            .capabilities()
            .is_supported(RuntimeCapability::HotGuests)
        {
            tracing::warn!(
                profile = %name,
                "the runtime backend does not hold a machine open; this allocation clones"
            );
            return None;
        }
        Some(HotContext {
            hot,
            name: name.clone(),
            lane,
            fingerprint: self.inner.worker.base_fingerprint(&profile.template).await,
            warm_generation: self.promoted_generation(name).await,
            max_hot_vms: config.runtime.max_hot_vms,
            age_seconds,
        })
    }

    /// The generation the profile currently promotes, so a machine built from
    /// a superseded warm image drains instead of serving another job.
    pub(super) async fn promoted_generation(&self, name: &ProfileName) -> Option<u64> {
        self.inner
            .images
            .load(name)
            .await
            .ok()
            .flatten()
            .filter(|record| record.state == flanforge_core::WarmImageState::Promoted)
            .map(|record| record.generation)
    }

    /// Every hot record, projected for the operator surface. Reports only.
    pub async fn hot_list(&self) -> Vec<HotGuestStatus> {
        let now = unix_time();
        let guests = self.inner.hot.load_all().await.unwrap_or_else(|error| {
            tracing::error!(%error, "hot guest listing is unavailable");
            Vec::new()
        });
        let present = self.hot_present_names().await;
        let mut statuses = guests
            .into_iter()
            .map(|guest| HotGuestStatus {
                age_seconds: now.saturating_sub(guest.booted_at_unix),
                idle_seconds: now.saturating_sub(guest.updated_at_unix),
                is_machine_present: present.contains(guest.vm_name.as_str()),
                vm_name: guest.vm_name,
                profile: guest.profile,
                lane: guest.lane,
                state: guest.state,
                jobs_served: guest.jobs_served,
                claimed_by: guest.claimed_by,
                drain_reason: guest.drain_reason,
            })
            .collect::<Vec<_>>();
        // Oldest first, so the machine closest to its bounds reads first.
        statuses.sort_by(|left, right| {
            right
                .age_seconds
                .cmp(&left.age_seconds)
                .then_with(|| left.vm_name.as_str().cmp(right.vm_name.as_str()))
        });
        statuses
    }

    async fn hot_present_names(&self) -> std::collections::BTreeSet<String> {
        self.host_machines()
            .await
            .map(|machines| machines.to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|machine| {
                machine.state == super::super::MachineState::Running
                    && machine.ownership == super::super::MachineOwnership::Owned
            })
            .map(|machine| machine.name)
            .collect()
    }
}
