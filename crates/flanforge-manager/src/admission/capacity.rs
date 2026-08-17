use std::collections::{BTreeSet, HashMap};

use flanforge_core::{Allocation, AllocationId, AllocationMode, Config, GuestSize, ProfileName};

use super::super::{
    AllocationManager, BusyReason, HostMachine, MachineState, ManagerError, service::Entry,
};

/// What the live entries already commit, folded fresh on every admission so
/// there is no counter to leak and no explicit release.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CommittedCapacity {
    pub(crate) active: usize,
    pub(crate) cpu_count: u32,
    pub(crate) memory_mb: u32,
}

impl AllocationManager {
    /// One producer per profile: two regenerations derive the same `<warm>`,
    /// `<warm>.staging`, and `<warm>.previous` names and would race on them.
    pub(crate) fn ensure_sole_producer(
        entries: &HashMap<AllocationId, Entry>,
        profile: &ProfileName,
        mode: AllocationMode,
    ) -> Result<(), ManagerError> {
        if mode != AllocationMode::Regenerate {
            return Ok(());
        }
        let holder = entries
            .values()
            .map(|entry| &entry.allocation)
            .find(|allocation| {
                !allocation.state.is_terminal()
                    && allocation.mode == AllocationMode::Regenerate
                    && &allocation.request.profile == profile
            });
        match holder {
            Some(holder) => Err(busy(BusyReason::Regenerating, Some(holder.id))),
            None => Ok(()),
        }
    }

    /// Admits under the running-VM limit and the host budget, or names why not.
    pub(crate) fn ensure_capacity(
        entries: &HashMap<AllocationId, Entry>,
        config: &Config,
        requested: GuestSize,
        machines: &[HostMachine],
    ) -> Result<(), ManagerError> {
        let active = active_allocations(entries);
        let holder = active.first().map(|allocation| allocation.id);
        let foreign = foreign_running(&active, machines);
        let committed = committed_capacity(&active, config);
        let orphaned = orphan_capacity(entries, &active, machines, config);
        if foreign + committed.active + 1 > usize::from(config.runtime.max_running_vms) {
            return Err(busy(BusyReason::Slots, holder));
        }
        let (Some(host_cpu_count), Some(host_memory_mb)) =
            (config.runtime.host_cpu_count, config.runtime.host_memory_mb)
        else {
            // An unset budget keeps the serialized behaviour that predates it.
            return if committed.active == 0 {
                Ok(())
            } else {
                Err(busy(BusyReason::Serialized, holder))
            };
        };
        let cpu_count = committed.cpu_count + orphaned.cpu_count + u32::from(requested.cpu_count);
        let memory_mb = committed
            .memory_mb
            .saturating_add(orphaned.memory_mb)
            .saturating_add(requested.memory_mb);
        if cpu_count > u32::from(host_cpu_count) || memory_mb > host_memory_mb {
            return Err(busy(BusyReason::Budget, holder));
        }
        Ok(())
    }
}

/// Machines the host reports running that no live entry owns; a box someone
/// started by hand takes a slot exactly like one of ours.
pub(crate) fn foreign_running(active: &[&Allocation], machines: &[HostMachine]) -> usize {
    let owned = active
        .iter()
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    machines
        .iter()
        .filter(|machine| machine.state == MachineState::Running)
        .filter(|machine| !owned.contains(machine.name.as_str()))
        .count()
}

pub(crate) fn active_allocations(entries: &HashMap<AllocationId, Entry>) -> Vec<&Allocation> {
    entries
        .values()
        .map(|entry| &entry.allocation)
        .filter(|allocation| !allocation.state.is_terminal())
        .collect()
}

/// A guest still running under a terminal record is memory the host has already
/// spent, so it is charged as well as counted. A machine no record claims stays
/// slot-only: a box someone started by hand is not the daemon's to size.
fn orphan_capacity(
    entries: &HashMap<AllocationId, Entry>,
    active: &[&Allocation],
    machines: &[HostMachine],
    config: &Config,
) -> CommittedCapacity {
    let owned = active
        .iter()
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    let orphans = machines
        .iter()
        .filter(|machine| machine.state == MachineState::Running)
        .filter(|machine| !owned.contains(machine.name.as_str()))
        .filter_map(|machine| {
            entries
                .values()
                .map(|entry| &entry.allocation)
                .find(|held| {
                    held.state.is_terminal()
                        && held.vm_created
                        && held.vm_name.as_str() == machine.name
                })
        })
        .collect::<Vec<_>>();
    committed_capacity(&orphans, config)
}

/// A record written before per-request sizing charges its profile's current
/// values, and one whose profile is also gone charges the largest profile
/// configured: a guest of unknown size is not a guest that costs nothing.
pub(crate) fn committed_capacity(active: &[&Allocation], config: &Config) -> CommittedCapacity {
    active
        .iter()
        .filter_map(|allocation| {
            allocation
                .size
                .or_else(|| profile_size(config, &allocation.request.profile))
                .or_else(|| largest_profile_size(config))
        })
        .fold(
            CommittedCapacity {
                active: active.len(),
                ..CommittedCapacity::default()
            },
            |total, size| CommittedCapacity {
                active: total.active,
                cpu_count: total.cpu_count + u32::from(size.cpu_count),
                memory_mb: total.memory_mb.saturating_add(size.memory_mb),
            },
        )
}

fn profile_size(config: &Config, name: &ProfileName) -> Option<GuestSize> {
    config.profiles.get(name).map(|profile| GuestSize {
        cpu_count: profile.cpu_count,
        memory_mb: profile.memory_mb,
    })
}

/// The defensible floor for an unknown guest: the budget must already cover it.
fn largest_profile_size(config: &Config) -> Option<GuestSize> {
    config
        .profiles
        .values()
        .map(|profile| GuestSize {
            cpu_count: profile.cpu_count,
            memory_mb: profile.memory_mb,
        })
        .max_by_key(|size| (size.cpu_count, size.memory_mb))
}

pub(crate) fn busy(reason: BusyReason, holder: Option<AllocationId>) -> ManagerError {
    tracing::warn!(?reason, ?holder, "allocation capacity is busy");
    ManagerError::Busy { reason, holder }
}
