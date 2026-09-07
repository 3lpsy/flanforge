use std::collections::BTreeSet;

use flanforge_core::{Allocation, Config, HotGuest, HotState};
use flanforge_store::StoreError;

use super::super::{BusyReason, HostMachine, MachineOwnership, MachineState, ManagerError};
use super::capacity::{CommittedCapacity, busy};

/// The names the pool actually holds: a live record whose machine the host
/// reports running and owned. Ownership is re-checked here for the reason
/// RUN-727 re-checks it for an allocation's name — a colliding name is not
/// evidence, so a record naming a machine this service does not own leaves
/// that machine to the fail-closed foreign path.
pub(crate) fn hot_names(hot: &[HotGuest], machines: &[HostMachine]) -> BTreeSet<String> {
    let owned = machines
        .iter()
        .filter(|machine| {
            machine.state == MachineState::Running && machine.ownership == MachineOwnership::Owned
        })
        .map(|machine| machine.name.as_str())
        .collect::<BTreeSet<_>>();
    hot.iter()
        .filter(|guest| guest.state.is_holding_machine())
        .map(|guest| guest.vm_name.to_string())
        .filter(|name| owned.contains(name.as_str()))
        .collect()
}

/// Turns an unreadable hot store into a refusal only when the live
/// configuration enables hot somewhere. Without a pool nobody turned on there
/// is nothing to misclassify, so a missing snapshot is logged and admission
/// proceeds on the pre-hot classification.
pub(crate) fn hot_snapshot(
    loaded: Result<Vec<HotGuest>, StoreError>,
    config: &Config,
) -> Result<Vec<HotGuest>, ManagerError> {
    match loaded {
        Ok(hot) => Ok(hot),
        Err(error) if is_hot_configured(config) => {
            tracing::error!(%error, "hot guest records are unreadable; a pool machine cannot be told from a foreign one");
            Err(busy(BusyReason::ProbeFailed, None))
        }
        Err(error) => {
            tracing::warn!(%error, "hot guest records are unreadable, but no profile enables hot");
            Ok(Vec::new())
        }
    }
}

fn is_hot_configured(config: &Config) -> bool {
    config.runtime.max_hot_vms > 0
        || config
            .profiles
            .values()
            .any(|profile| profile.hot.is_some_and(|hot| hot.enabled))
}

/// A slot and a size for every machine the pool holds or has reserved, so
/// nothing it claims goes uncharged.
///
/// Two arms. `hot_names` only names machines the host already reports running
/// and owned; a record is written at the moment a finished allocation's guest
/// is handed over, which is before the next listing has to agree. Charging
/// only the listed arm would leave a machine mid-handover uncharged, so a
/// `Provisioning` record is charged from the moment it is written.
///
/// A machine an active allocation already names is skipped, because
/// `committed_capacity` charges it through that allocation and charging both
/// double-counts the host. The skip reads the live allocation, not `HotState`:
/// a record left `Claimed` by an allocation that has since gone terminal is
/// charged here.
///
/// The size is the record's own, taken from the allocation that ran on it, so
/// a machine retained under a superseded profile is charged what it actually
/// costs rather than the profile's current values.
pub(crate) fn hot_capacity(
    hot: &[HotGuest],
    active: &[&Allocation],
    hot_names: &BTreeSet<String>,
) -> CommittedCapacity {
    let claimed = active
        .iter()
        .flat_map(|allocation| {
            // Both names an allocation can hold: the guest it cloned, and the
            // pool machine it reused. Either one means `committed_capacity`
            // has already charged this host for it.
            std::iter::once(allocation.vm_name.as_str()).chain(
                allocation
                    .origin
                    .hot_vm_name()
                    .map(flanforge_core::VmName::as_str),
            )
        })
        .collect::<BTreeSet<_>>();
    hot.iter()
        .filter(|guest| guest.state.is_holding_machine())
        .filter(|guest| {
            hot_names.contains(guest.vm_name.as_str()) || guest.state == HotState::Provisioning
        })
        .filter(|guest| !claimed.contains(guest.vm_name.as_str()))
        .fold(CommittedCapacity::default(), |total, guest| {
            CommittedCapacity {
                active: total.active + 1,
                cpu_count: total
                    .cpu_count
                    .saturating_add(u32::from(guest.size.cpu_count)),
                memory_mb: total.memory_mb.saturating_add(guest.size.memory_mb),
                storage_mb: total.storage_mb.saturating_add(guest.size.storage_mb),
            }
        })
}
