use async_trait::async_trait;
use flanforge_core::{
    Allocation, BaseFingerprint, CloneSource, HotGuest, Profile, ProfileName, VmName,
    WarmImageRecord,
};
use flanforge_wire::{RuntimeCapabilities, RuntimeHealth, RuntimeStatus};
use tokio_util::sync::CancellationToken;

use super::{
    AllocationReporter, CleanupBudget, HostMachine, HotReset, HotRetainRequest, ImageSweep,
    MachineState, ReapRequest, RetiredImage, WarmAvailability, WorkerError,
};

#[async_trait]
pub trait AllocationWorker: std::fmt::Debug + Send + Sync {
    async fn run(
        &self,
        allocation: Allocation,
        profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError>;

    /// Whether admission must leave clone provenance unset until the backend
    /// records the source it actually cloned.
    fn is_clone_source_evidence_deferred(&self) -> bool {
        false
    }

    /// Tears one allocation's guest and runner registration down inside
    /// `budget`. That budget carries the profile's configured allowance
    /// already, capped by whatever a running shutdown has left, and it is the
    /// only teardown clock a backend may read.
    async fn cleanup(
        &self,
        allocation: Allocation,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError>;

    /// Every machine the host reports, regardless of owner.
    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(Vec::new())
    }

    /// Capability set, static per backend and free of I/O — libvirt's
    /// `runtime_status()` spawns a helper and must not run per allocation.
    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::tart()
    }

    /// Backend identity, capabilities, and best-effort health for operators.
    async fn runtime_status(&self) -> RuntimeStatus {
        RuntimeStatus::tart(RuntimeHealth::Healthy, None)
    }

    /// Fingerprints a template so a rebuilt base invalidates warm images.
    async fn base_fingerprint(&self, _template: &VmName) -> Option<BaseFingerprint> {
        None
    }

    /// The virtual size, in MiB, of the base this source boots from.
    ///
    /// An ephemeral guest's disk is never smaller than the immutable base
    /// underneath it, so admission charges that floor rather than the declared
    /// `storage_mb`. `None` where the backend cannot see the base at all,
    /// which leaves the declared size charged.
    async fn base_storage_mb(&self, _profile: &ProfileName, _source: &CloneSource) -> Option<u64> {
        None
    }

    /// Is this record's warm image usable for this profile right now?
    ///
    /// The default is the name-addressed answer: look the recorded warm name
    /// up in `listing`, the host listing the manager already cached, so
    /// resolving a source never spawns a redundant subprocess. A
    /// pointer-addressed backend answers from its own durable state and
    /// ignores the argument.
    ///
    /// # Errors
    /// Returns an error when host state cannot be read.
    async fn warm_availability(
        &self,
        _profile: &Profile,
        record: &WarmImageRecord,
        listing: &[HostMachine],
    ) -> Result<WarmAvailability, WorkerError> {
        Ok(listing
            .iter()
            .find(|machine| machine.name == record.warm_template.as_str())
            .map_or(WarmAvailability::Absent, |machine| {
                if machine.state == MachineState::Stopped {
                    WarmAvailability::Ready
                } else {
                    WarmAvailability::Busy
                }
            }))
    }

    /// Is an image already sitting under this declared name that the daemon
    /// never recorded? Only a name-addressed backend can answer yes, and it
    /// answers from the listing the caller already holds.
    ///
    /// # Errors
    /// Returns an error when host state cannot be read.
    async fn is_unclaimed_image(
        &self,
        name: &VmName,
        listing: &[HostMachine],
    ) -> Result<bool, WorkerError> {
        Ok(listing.iter().any(|machine| machine.name == name.as_str()))
    }

    /// Superseded generations still pinned by a live or recoverable consumer.
    ///
    /// Answered from durable state only: `daemon status` must not pay for a
    /// pool walk. `None` means the backend retains no generations at all.
    async fn warm_retained(&self, _profile: &ProfileName) -> Option<u32> {
        None
    }

    /// Backend crash reconciliation, run once at daemon start.
    ///
    /// # Errors
    /// Returns an error when durable backend state cannot be reconciled.
    async fn ensure_recovered(&self) -> Result<(), WorkerError> {
        Ok(())
    }

    /// Deletes a VM the reaper authorized; the runtime re-checks ownership.
    async fn delete_vm(&self, _request: ReapRequest<'_>) -> Result<(), WorkerError> {
        Err(WorkerError::new("deletion is not supported by this worker"))
    }

    /// Restores the generation that survives an interrupted promotion, before
    /// the record reverts to it.
    ///
    /// A name-addressed backend clones `<warm>.previous` back over `<warm>`; a
    /// pointer-addressed backend repoints at the surviving generation. Neither
    /// may remove the live image: a pointer with nothing to restore is left
    /// alone and logged, because the record reverting is what costs a cold
    /// boot while removing a good image costs the image.
    async fn ensure_warm_restored(
        &self,
        _profile: &Profile,
        _record: &WarmImageRecord,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::new(
            "warm image restore is not supported by this worker",
        ))
    }

    /// Takes a finished allocation's guest into the pool: keeps the machine
    /// alive past the worker that built it, and leaves it reset.
    ///
    /// This is the only way a machine enters the pool. Nothing provisions one,
    /// so a backend has no "build me a guest" path to implement — only a
    /// "keep the one you have" path.
    ///
    /// The manager owns the record: this reports success or failure and writes
    /// nothing durable of its own. A failure is not a failed job; the guest is
    /// torn down the way a non-hot allocation's is.
    ///
    /// # Errors
    /// Returns an error when the machine cannot be kept or does not come back
    /// clean.
    async fn ensure_hot_retained(&self, _request: HotRetainRequest<'_>) -> Result<(), WorkerError> {
        Err(WorkerError::new(
            "hot guests are not supported by this worker",
        ))
    }

    /// Runs the recycle gate over a machine whose job has ended. Anything but
    /// a clean pass fails the machine, and the next allocation clones.
    ///
    /// # Errors
    /// Returns an error when the gate times out, refuses, or cannot be reached.
    async fn ensure_hot_reset(
        &self,
        _guest: &HotGuest,
        _reset: HotReset,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::new(
            "hot guests are not supported by this worker",
        ))
    }

    /// Is this machine still up and reachable? A bounded probe, seconds not
    /// minutes: it is the one check a claim still pays for.
    async fn is_hot_live(&self, _guest: &HotGuest) -> bool {
        false
    }

    /// Destroys a pool machine. This is the only path that deletes one — a hot
    /// machine never reaches `delete_vm`, because the reaper's authorizations
    /// answer "nothing references this name" and a hot machine is referenced by
    /// its own record. Taking `&HotGuest` rather than a name is what makes
    /// eviction without a record unrepresentable.
    ///
    /// # Errors
    /// Returns an error when the machine cannot be destroyed inside `budget`.
    async fn ensure_hot_evicted(
        &self,
        _guest: &HotGuest,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::new(
            "hot guests are not supported by this worker",
        ))
    }

    /// Retires superseded warm generations. Reports on a dry run, deletes only
    /// when asked.
    ///
    /// # Errors
    /// Returns an error when the reference proof cannot be completed.
    async fn sweep_images(
        &self,
        _request: ImageSweep<'_>,
    ) -> Result<Vec<RetiredImage>, WorkerError> {
        Ok(Vec::new())
    }
}
