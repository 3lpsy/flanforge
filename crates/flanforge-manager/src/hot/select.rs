use std::collections::BTreeSet;

use flanforge_core::{
    BaseFingerprint, HotConfig, HotDrainReason, HotGuest, HotLane, HotState, ProfileName,
    is_fingerprint_match,
};

/// What a claim decision needs to know that the record itself cannot say: the
/// profile's live bounds, and what the profile currently resolves to. All of
/// it is read before the entries lock and folded under it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HotPolicy<'a> {
    pub(crate) hot: &'a HotConfig,
    /// The base the profile resolves to right now. `None` where the backend
    /// cannot fingerprint at all, which is not evidence of a mismatch.
    pub(crate) fingerprint: Option<&'a BaseFingerprint>,
    /// The warm generation the profile currently promotes, if any.
    pub(crate) warm_generation: Option<u64>,
    pub(crate) now: u64,
}

/// Why this machine may no longer take a claim, or `None` while it may.
///
/// Order matters only for the reason reported: every arm ends the same way.
/// The two fingerprint arms are the non-configurable bounds — correctness, not
/// policy — so they are checked whatever the table says.
pub(crate) fn drain_reason(guest: &HotGuest, policy: HotPolicy<'_>) -> Option<HotDrainReason> {
    if guest.state.is_terminal() {
        return None;
    }
    // The run that retained the machine set its lifetime; the profile's
    // `max_lifetime_seconds` is the ceiling on that, so the lower of the two
    // binds and a reload that lowers the ceiling shortens a machine already
    // running under the old one.
    let limit = guest
        .age_limit_seconds
        .unwrap_or(policy.hot.max_lifetime_seconds)
        .min(policy.hot.max_lifetime_seconds);
    if policy.now.saturating_sub(guest.booted_at_unix) >= limit {
        return Some(HotDrainReason::MaxLifetime);
    }
    if u64::from(guest.jobs_served) >= u64::from(policy.hot.max_jobs) {
        return Some(HotDrainReason::MaxJobs);
    }
    // Only a base the machine was itself fingerprinted against can be
    // compared. Warm selection reads an unknown fingerprint as "do not use the
    // image", because falling back to a cold boot costs one boot; here the
    // same reading would drain every machine on a backend that cannot
    // fingerprint at all, so hot never works. Unknown is not evidence of a
    // change, and a machine only drains on a fingerprint that actually moved.
    let is_base_changed = match (guest.source.base_fingerprint.as_ref(), policy.fingerprint) {
        (Some(recorded), Some(current)) => !is_fingerprint_match(Some(recorded), Some(current)),
        _ => false,
    };
    if is_base_changed || guest.warm_generation != policy.warm_generation {
        return Some(HotDrainReason::StaleBase);
    }
    // The idle clock runs from the last state change, which is when the
    // machine became claimable; a claimed machine is not idling.
    if guest.state == HotState::Idle
        && policy.now.saturating_sub(guest.updated_at_unix) >= policy.hot.idle_ttl_seconds
    {
        return Some(HotDrainReason::IdleTtl);
    }
    None
}

/// The one machine a claim may take, or `None`.
///
/// Pure by construction: the caller holds the entries lock, and the set of
/// names live entries already claim is what makes two concurrent creates
/// unable to pick the same machine. Nothing here reads a store or a host.
///
/// The machine that has served most jobs wins, so wear concentrates and the
/// pool retires machines one at a time instead of all at once. The name breaks
/// ties, so the choice is deterministic and testable.
pub(crate) fn claimable<'a>(
    hot: &'a [HotGuest],
    profile: &ProfileName,
    lane: HotLane,
    claimed: &BTreeSet<&str>,
    policy: HotPolicy<'_>,
) -> Option<&'a HotGuest> {
    hot.iter()
        .filter(|guest| guest.is_serving(profile, lane))
        .filter(|guest| !claimed.contains(guest.vm_name.as_str()))
        .filter(|guest| drain_reason(guest, policy).is_none())
        .max_by(|left, right| {
            left.jobs_served
                .cmp(&right.jobs_served)
                .then_with(|| right.vm_name.as_str().cmp(left.vm_name.as_str()))
        })
}

/// Whether the pool has room for one more machine, globally.
///
/// Only ever asked on behalf of an allocation that has already failed to find
/// an idle machine, and only to decide whether that allocation's own guest may
/// be retained when its job ends. Nothing calls this to top the pool up: a
/// machine enters the pool as the residue of a completed hot allocation and no
/// other way.
///
/// This is the global cap alone. `max_idle` is a per-lane ceiling applied at
/// release by `surplus_idle`, because that is the moment a machine actually
/// joins a lane — and the precedence between the two is stated once, here:
/// `runtime.max_hot_vms` is a fact about the host, so it always wins. A
/// `max_idle` above it is reachable in arithmetic and never on the host, which
/// is why configuration advises against it rather than refusing it.
///
/// `in_flight` names allocations that will retain a machine but have not yet,
/// which the record snapshot read before the entries lock cannot show.
pub(crate) fn is_pool_expandable(
    hot: &[HotGuest],
    max_hot_vms: u8,
    in_flight: &[&ProfileName],
) -> bool {
    let held = hot
        .iter()
        .filter(|guest| guest.state.is_holding_machine())
        .count()
        + in_flight.len();
    held < usize::from(max_hot_vms)
}

/// Idle machines this lane holds beyond `max_idle`, oldest-updated first, so a
/// release that overshoots retires the stalest machine rather than the one
/// just recycled: the freshest machine is the one whose caches are warmest.
pub(crate) fn surplus_idle<'a>(
    hot: &'a [HotGuest],
    profile: &ProfileName,
    lane: HotLane,
    hot_config: &HotConfig,
) -> Vec<&'a HotGuest> {
    let mut idle = hot
        .iter()
        .filter(|guest| guest.is_serving(profile, lane))
        .collect::<Vec<_>>();
    idle.sort_by(|left, right| {
        left.updated_at_unix
            .cmp(&right.updated_at_unix)
            .then_with(|| left.vm_name.as_str().cmp(right.vm_name.as_str()))
    });
    let keep = usize::from(hot_config.max_idle);
    idle.truncate(idle.len().saturating_sub(keep));
    idle
}
