use std::collections::{BTreeMap, BTreeSet};

use flanforge_core::{
    Allocation, GuestSize, HotGuest, Profile, ProfileName, SimulatorReset, VmName, WarmImageRecord,
};
use serde::{Deserialize, Serialize};

use super::super::reaper::ReapAuthorization;
use super::CleanupBudget;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Running,
    Stopped,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineOwnership {
    Owned,
    Foreign,
    Unknown,
}

/// One machine the host reports, whoever owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostMachine {
    pub name: String,
    pub state: MachineState,
    /// From the VM directory mtime; None when `tart_home` is unset.
    pub age_seconds: Option<u64>,
    /// Live resources when the backend can report all three dimensions.
    pub size: Option<GuestSize>,
    /// The strongest ownership evidence the backend has. libvirt derives it
    /// from domain metadata, which a name cannot forge. Tart has no per-VM
    /// metadata, so it uses the configured prefix — the same authority that
    /// already gates every Tart delete, and the reason a colliding name is
    /// only reachable by someone who can already create VMs as the service
    /// user.
    pub ownership: MachineOwnership,
}

/// Whether one profile's recorded warm image can be booted right now.
///
/// A name-addressed backend answers from its VM listing; a pointer-addressed
/// backend answers from its own durable pointer. Both fail toward a cold boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarmAvailability {
    Ready,
    /// Present but not in a state a clone or a boot may use.
    Busy,
    Absent,
}

/// One warm generation a sweep considered for retirement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetiredImage {
    pub profile: ProfileName,
    pub generation: u64,
    pub age_seconds: u64,
    /// False on a dry run, and false when the reference proof refused.
    pub is_deleted: bool,
    pub reason: String,
}

/// What a sweep may act on, computed by the manager so a backend never has to
/// read configuration or records itself.
#[derive(Clone, Copy, Debug)]
pub struct ImageSweep<'a> {
    pub is_dry_run: bool,
    /// Warm names live configuration still declares, by profile.
    pub declared: &'a BTreeMap<ProfileName, VmName>,
    /// Profiles a durable warm record still claims, after pruning.
    pub claimed: &'a BTreeSet<ProfileName>,
}

/// A deletion the reaper proposes, carrying the evidence the runtime re-checks.
#[derive(Clone, Copy, Debug)]
pub struct ReapRequest<'a> {
    pub name: &'a VmName,
    pub authorization: &'a ReapAuthorization,
    /// The profile that declares the name, when one still does.
    pub profile: Option<&'a Profile>,
    /// The daemon's own record claiming the name, for a repointed image.
    pub record: Option<&'a WarmImageRecord>,
    /// The terminal allocation a `Record` authorization rests on. Tearing the
    /// guest down without it leaves the guest's Forgejo registration behind.
    pub allocation: Option<&'a Allocation>,
    /// Every image name live configuration claims; none of them is deletable,
    /// whatever a record says, and whether or not its profile still exists.
    pub reserved: &'a BTreeSet<String>,
    /// Teardown budget for a backend that has to stop, undefine, and delete
    /// volumes. The named profile's own allowance where one names the VM, and
    /// the widest configured profile's otherwise — an orphan no record claims
    /// still needs more than the shared minimum, least of all over a remote
    /// connection. A running shutdown caps it, as it caps every teardown.
    pub budget: CleanupBudget,
}

/// What the recycle gate is allowed to do and how long it has.
///
/// The budget is a `CleanupBudget` rather than a type of its own: the gate is
/// the same shape of bounded guest operation a teardown is, and it must be
/// capped by a running shutdown for the same reason.
#[derive(Clone, Copy, Debug)]
pub struct HotReset {
    pub simulator_reset: SimulatorReset,
    pub budget: CleanupBudget,
}

/// One finished allocation's guest, handed to the pool instead of destroyed.
///
/// The pool never provisions: a machine enters it only as the residue of a
/// completed hot allocation, which is what this asks the backend to keep. The
/// manager owns the record and has already written it; the backend's job is to
/// keep the machine alive past the worker that built it and leave it reset.
#[derive(Clone, Copy, Debug)]
pub struct HotRetainRequest<'a> {
    /// The record now naming the machine, written before the handover so a
    /// machine the pool holds is never a machine with no record.
    pub guest: &'a HotGuest,
    /// The allocation whose job just ended, which is what the backend knows
    /// the running guest by.
    pub allocation: &'a Allocation,
    pub profile: &'a Profile,
    /// The recycle gate's settings, run as part of the handover so a retained
    /// machine and a recycled one enter the pool in the same state.
    pub reset: HotReset,
}
