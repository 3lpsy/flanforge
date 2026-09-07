use std::collections::{BTreeSet, HashMap};

use flanforge_core::{
    Allocation, AllocationId, AllocationOrigin, CloneSource, HotGuest, HotLane, ProfileName,
};

use super::{
    super::service::Entry,
    model::HotContext,
    select::{claimable, is_pool_expandable},
};

/// What admission decided about one `hot` request.
///
/// There is no third arm for "provision a machine": the pool is populated only
/// as the residue of a completed hot allocation, so a request that finds no
/// idle machine simply runs as an ordinary allocation whose teardown retains
/// the guest it already had.
#[derive(Clone, Debug)]
pub(crate) enum HotClaim {
    /// An idle pool machine, already moved to `Claimed` and ready to commit.
    Reused {
        guest: Box<HotGuest>,
        origin: AllocationOrigin,
        /// The source the reused machine was itself built from, copied onto
        /// the allocation so provenance reads whole rather than half from each
        /// record.
        source: CloneSource,
    },
    /// No machine to reuse; this allocation clones as usual and its guest is
    /// kept when the job ends.
    Retain,
}

/// Decides, under the caller's entries lock, whether this allocation reuses a
/// pool machine and whether its own guest is retained afterwards.
///
/// Pure by construction: it reads the snapshot loaded before the lock plus the
/// names live entries already claim, and touches no store and no host. The
/// exclusion set is built from the entries themselves, so two `create` calls
/// folding under the same lock cannot pick one machine: the first inserts its
/// entry before releasing the lock, and the second sees it.
pub(crate) fn plan_hot_claim(
    entries: &HashMap<AllocationId, Entry>,
    hot: &[HotGuest],
    context: &HotContext,
    id: AllocationId,
) -> Option<HotClaim> {
    let claimed = live_hot_names(entries);
    if let Some(guest) = claimable(hot, &context.name, context.lane, &claimed, context.policy()) {
        let mut guest = guest.clone();
        // `ensure_claimed` refuses anything but `Idle`, so the one shape that
        // would put two allocations on one machine cannot be built.
        guest.ensure_claimed(id).ok()?;
        // The claiming run sets the machine's lifetime, so a longer request
        // extends a machine a shorter one retained. The ceiling still binds:
        // `drain_reason` takes the lower of this and the profile's.
        if context.age_seconds.is_some() {
            guest.age_limit_seconds = context.age_seconds;
        }
        return Some(HotClaim::Reused {
            origin: AllocationOrigin::HotReuse {
                vm_name: guest.vm_name.clone(),
                jobs_served: guest.jobs_served,
                booted_at_unix: guest.booted_at_unix,
            },
            source: guest.source.clone(),
            guest: Box::new(guest),
        });
    }
    // Retention is the only way a machine enters the pool, so the global cap
    // is checked here rather than at release: a request the pool has no room
    // for is refused now, while the answer can still be recorded.
    is_pool_expandable(
        hot,
        context.max_hot_vms,
        &in_flight_retentions(entries, hot),
    )
    .then_some(HotClaim::Retain)
}

/// Allocations that will retain a machine but have not yet, so the cap counts
/// them. The record snapshot was read before the lock and cannot show them.
fn in_flight_retentions<'a>(
    entries: &'a HashMap<AllocationId, Entry>,
    hot: &[HotGuest],
) -> Vec<&'a ProfileName> {
    let recorded = hot
        .iter()
        .filter(|guest| guest.state.is_holding_machine())
        .map(|guest| guest.vm_name.as_str())
        .collect::<BTreeSet<_>>();
    entries
        .values()
        .map(|entry| &entry.allocation)
        .filter(|allocation| !allocation.state.is_terminal())
        .filter(|allocation| allocation.hot_lane.is_some())
        // The machine an allocation will leave behind is the pool machine it
        // reused, when it reused one, and its own guest otherwise. A reusing
        // allocation's `vm_name` is a fresh name that never becomes a machine,
        // so charging it would count the host twice.
        .filter(|allocation| {
            let held = allocation
                .origin
                .hot_vm_name()
                .map_or(allocation.vm_name.as_str(), flanforge_core::VmName::as_str);
            !recorded.contains(held)
        })
        .map(|allocation| &allocation.request.profile)
        .collect()
}

/// Names every non-terminal entry claims through its origin. This is the
/// exclusion set that makes the claim safe, and it lives in the entries rather
/// than in the durable records because the entries are what the lock protects.
pub(crate) fn live_hot_names(entries: &HashMap<AllocationId, Entry>) -> BTreeSet<&str> {
    entries
        .values()
        .map(|entry| &entry.allocation)
        .filter(|allocation| !allocation.state.is_terminal())
        .filter_map(|allocation| allocation.origin.hot_vm_name())
        .map(flanforge_core::VmName::as_str)
        .collect()
}

/// The machine a retained allocation leaves behind, as a fresh pool record.
/// Written at release rather than before the clone: under this model the
/// machine already exists and has already served the job that earned it.
pub(crate) fn retained_record(allocation: &Allocation, lane: HotLane) -> Option<HotGuest> {
    let mut guest = HotGuest::new(
        allocation.vm_name.clone(),
        allocation.request.profile.clone(),
        lane,
        allocation.size?,
        allocation.source.clone()?,
        allocation.warm_generation,
        allocation.hot_age_seconds,
    );
    // The job this allocation just ran is the machine's first.
    guest.jobs_served = 1;
    Some(guest)
}
