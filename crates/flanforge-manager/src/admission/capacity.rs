use std::collections::{BTreeSet, HashMap};

use flanforge_core::{
    Allocation, AllocationId, AllocationMode, Config, GuestSize, HotGuest, ProfileName,
};

use super::{
    super::{
        AllocationManager, BusyReason, HostMachine, MachineOwnership, MachineState, ManagerError,
        service::Entry,
    },
    hot,
};

/// What the live entries already commit, folded fresh on every admission so
/// there is no counter to leak and no explicit release.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CommittedCapacity {
    pub(crate) active: usize,
    pub(crate) cpu_count: u32,
    pub(crate) memory_mb: u32,
    pub(crate) storage_mb: u64,
}

impl AllocationManager {
    /// One producer per profile: two regenerations compute the same next
    /// generation from the same record and would race on the image it names.
    /// That holds however the backend addresses its images — a name-addressed
    /// one races on the derived names, a pointer-addressed one on the pointer
    /// document — so immutable physical names do not make this relaxable.
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
    /// A hot machine is neither foreign nor an orphan: it is classified from
    /// its record and charged as the slot and size it holds continuously.
    pub(crate) fn ensure_capacity(
        entries: &HashMap<AllocationId, Entry>,
        config: &Config,
        requested: GuestSize,
        machines: &[HostMachine],
        hot: &[HotGuest],
    ) -> Result<(), ManagerError> {
        let active = active_allocations(entries);
        let holder = active.first().map(|allocation| allocation.id);
        let hot_names = hot::hot_names(hot, machines);
        let foreign = foreign_running(&active, machines, &hot_names);
        let committed = committed_capacity(&active, config);
        let orphaned = orphan_capacity(entries, &active, machines, config, &hot_names);
        let pooled = hot::hot_capacity(hot, &active, &hot_names);
        let (foreign_capacity, has_unknown_foreign_size) =
            foreign_capacity(entries, &active, machines, &hot_names);
        if foreign + committed.active + pooled.active + 1
            > usize::from(config.runtime.max_running_vms)
        {
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
        if has_unknown_foreign_size {
            return Err(busy(BusyReason::Budget, holder));
        }
        let cpu_count = committed
            .cpu_count
            .saturating_add(orphaned.cpu_count)
            .saturating_add(foreign_capacity.cpu_count)
            .saturating_add(pooled.cpu_count)
            .saturating_add(u32::from(requested.cpu_count));
        let memory_mb = committed
            .memory_mb
            .saturating_add(orphaned.memory_mb)
            .saturating_add(foreign_capacity.memory_mb)
            .saturating_add(pooled.memory_mb)
            .saturating_add(requested.memory_mb);
        let storage_mb = committed
            .storage_mb
            .saturating_add(orphaned.storage_mb)
            .saturating_add(foreign_capacity.storage_mb)
            .saturating_add(pooled.storage_mb)
            .saturating_add(requested.storage_mb);
        let storage_reserve = config
            .runtime
            .libvirt()
            .map_or(0, |libvirt| libvirt.min_storage_free_mb);
        let reserved_storage_mb = storage_mb.saturating_add(storage_reserve);
        if cpu_count > u32::from(host_cpu_count)
            || memory_mb > host_memory_mb
            || config
                .runtime
                .host_storage_mb
                .is_some_and(|host| reserved_storage_mb > host)
        {
            return Err(busy(BusyReason::Budget, holder));
        }
        Ok(())
    }
}

/// Charges known foreign workloads and marks unknown ones fail-closed. A
/// terminal allocation's surviving guest is charged by `orphan_capacity`.
fn foreign_capacity(
    entries: &HashMap<AllocationId, Entry>,
    active: &[&Allocation],
    machines: &[HostMachine],
    hot_names: &BTreeSet<String>,
) -> (CommittedCapacity, bool) {
    let active_names = active
        .iter()
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    let terminal_names = entries
        .values()
        .map(|entry| &entry.allocation)
        .filter(|allocation| allocation.state.is_terminal() && allocation.vm_created)
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    let mut capacity = CommittedCapacity::default();
    let mut unknown = false;
    for machine in machines.iter().filter(|machine| {
        machine.state == MachineState::Running
            && !is_owned_name(machine, &active_names)
            && !is_owned_name(machine, &terminal_names)
            && !hot_names.contains(&machine.name)
    }) {
        if let Some(size) = machine.size {
            capacity.active += 1;
            capacity.cpu_count = capacity.cpu_count.saturating_add(u32::from(size.cpu_count));
            capacity.memory_mb = capacity.memory_mb.saturating_add(size.memory_mb);
            capacity.storage_mb = capacity.storage_mb.saturating_add(size.storage_mb);
        } else {
            unknown = true;
        }
    }
    (capacity, unknown)
}

/// Machines the host reports running that no live entry owns; a box someone
/// started by hand takes a slot exactly like one of ours.
pub(crate) fn foreign_running(
    active: &[&Allocation],
    machines: &[HostMachine],
    hot_names: &BTreeSet<String>,
) -> usize {
    let owned = active
        .iter()
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    machines
        .iter()
        .filter(|machine| machine.state == MachineState::Running)
        .filter(|machine| !is_owned_name(machine, &owned))
        .filter(|machine| !hot_names.contains(&machine.name))
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
    hot_names: &BTreeSet<String>,
) -> CommittedCapacity {
    let owned = active
        .iter()
        .map(|allocation| allocation.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    let orphans = machines
        .iter()
        .filter(|machine| machine.state == MachineState::Running)
        .filter(|machine| !is_owned_name(machine, &owned))
        .filter(|machine| !hot_names.contains(&machine.name))
        .filter(|machine| machine.ownership == MachineOwnership::Owned)
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

fn is_owned_name(machine: &HostMachine, names: &BTreeSet<&str>) -> bool {
    machine.ownership == MachineOwnership::Owned && names.contains(machine.name.as_str())
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
                storage_mb: total.storage_mb.saturating_add(size.storage_mb),
            },
        )
}

fn profile_size(config: &Config, name: &ProfileName) -> Option<GuestSize> {
    config.profiles.get(name).map(|profile| GuestSize {
        cpu_count: profile.cpu_count,
        memory_mb: profile.memory_mb,
        storage_mb: profile.storage_mb,
    })
}

/// The defensible floor for an unknown guest: the budget must already cover it.
fn largest_profile_size(config: &Config) -> Option<GuestSize> {
    let mut profiles = config.profiles.values();
    let first = profiles.next()?;
    Some(profiles.fold(
        GuestSize {
            cpu_count: first.cpu_count,
            memory_mb: first.memory_mb,
            storage_mb: first.storage_mb,
        },
        |largest, profile| GuestSize {
            cpu_count: largest.cpu_count.max(profile.cpu_count),
            memory_mb: largest.memory_mb.max(profile.memory_mb),
            storage_mb: largest.storage_mb.max(profile.storage_mb),
        },
    ))
}

pub(crate) fn busy(reason: BusyReason, holder: Option<AllocationId>) -> ManagerError {
    tracing::warn!(?reason, ?holder, "allocation capacity is busy");
    ManagerError::Busy { reason, holder }
}
