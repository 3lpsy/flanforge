use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationState, BaseFingerprint, CloneSource, Config, Profile, ProfileName,
    VmName, WarmImageRecord,
};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::ForgejoClient;
use flanforge_manager::{
    AllocationReporter, AllocationWorker, CleanupBudget, HostMachine, HotReset, HotRetainRequest,
    ReapAuthorization, ReapRequest, WorkerError,
};
use flanforge_runtime::{GuestJob, RunnerDelivery};
use flanforge_wire::{RuntimeHealth, RuntimeStatus};

use super::{super::tart::TartClient, hot::HotPool};

#[derive(Clone, Debug)]
pub struct FlanForgeWorker {
    pub(crate) tart: TartClient,
    pub(crate) job: GuestJob,
    image_locks: Arc<ImageLocks>,
    /// Holds the `tart run` child of every machine the pool keeps. `tart run`
    /// is a child of `flanforged`, so this is what lets a guest outlive the
    /// worker that built it — and why a daemon restart takes the pool with it.
    pub(super) hot: Arc<HotPool>,
}

#[derive(Debug, Default)]
struct ImageLocks {
    profiles: Mutex<HashMap<ProfileName, Arc<Mutex<()>>>>,
    #[cfg(test)]
    attempts: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    attempted: tokio::sync::Notify,
}

impl FlanForgeWorker {
    /// Preserves the existing Tart constructor for callers that already
    /// selected and validated the backend.
    #[must_use]
    pub fn new(config: &Config, forgejo: ForgejoClient) -> Self {
        Self::try_new(config, forgejo).unwrap_or_else(|error| unreachable!("Tart worker: {error}"))
    }

    /// Builds the Tart backend from a configuration selected for Tart.
    ///
    /// # Errors
    ///
    /// Returns an error when composition attempts to use another backend.
    pub fn try_new(config: &Config, forgejo: ForgejoClient) -> Result<Self, WorkerError> {
        let tart = config
            .runtime
            .tart()
            .ok_or_else(|| WorkerError::new("Tart worker requires the Tart backend"))?;
        // No host path means the base image bakes the runner, as on libvirt.
        let delivery = match &tart.runner_host_path {
            Some(source) => RunnerDelivery::HostCopy(source.clone()),
            None => RunnerDelivery::Image,
        };
        Ok(Self {
            tart: TartClient::new(config.runtime.clone()),
            job: GuestJob::new(config, forgejo, delivery)?,
            image_locks: Arc::new(ImageLocks::default()),
            hot: Arc::new(HotPool::default()),
        })
    }

    pub(crate) async fn lock_profile_image(&self, profile: &ProfileName) -> OwnedMutexGuard<()> {
        #[cfg(test)]
        {
            self.image_locks
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.image_locks.attempted.notify_waiters();
        }
        let lock = self
            .image_locks
            .profiles
            .lock()
            .await
            .entry(profile.clone())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_image_lock_attempts(&self, expected: usize) {
        loop {
            let notified = self.image_locks.attempted.notified();
            if self
                .image_locks
                .attempts
                .load(std::sync::atomic::Ordering::SeqCst)
                >= expected
            {
                return;
            }
            notified.await;
        }
    }

    pub(crate) async fn phase<T, F>(
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
        timeout_message: &'static str,
        future: F,
    ) -> Result<T, WorkerError>
    where
        F: Future<Output = Result<T, WorkerError>>,
    {
        GuestJob::phase(cancellation, deadline, timeout_message, future).await
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_job_handle(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<String, WorkerError> {
        self.job
            .wait_for_job_handle(allocation, profile, cancellation, deadline)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn supervise(
        &self,
        started: flanforge_runtime::StartedRunner,
        allocation: &Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerError> {
        self.job
            .supervise(started, allocation, profile, reporter, cancellation)
            .await
    }
}

#[async_trait]
impl AllocationWorker for FlanForgeWorker {
    fn is_clone_source_evidence_deferred(&self) -> bool {
        true
    }

    async fn run(
        &self,
        mut allocation: Allocation,
        profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        tracing::info!(allocation_id = %allocation.id, profile = %allocation.request.profile, "allocation worker started");
        let boot_timeout = Duration::from_secs(profile.boot_timeout_seconds);
        // A reused machine is already up, so the clone, the boot, and the
        // readiness wait are all skipped and `Preparing` goes straight to
        // `Registering`. The manager proved it live before this was entered.
        let (mut vm, ip) = if let Some(pooled) = allocation.origin.hot_vm_name() {
            // The machine is already up and the manager just proved it live,
            // so this reads an address rather than waiting for a boot; the
            // boot budget is the right ceiling and it will not be spent.
            let address = self.tart.address_of(pooled, boot_timeout).await?;
            (None, address)
        } else {
            reporter
                .transition(AllocationState::Preparing)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
            let boot_deadline = tokio::time::Instant::now() + boot_timeout;
            let (vm, ip) = self
                .prepare_vm(
                    &mut allocation,
                    &profile,
                    &reporter,
                    &cancellation,
                    boot_deadline,
                    boot_timeout,
                )
                .await?;
            (Some(vm), ip)
        };
        reporter
            .transition(AllocationState::Registering)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let started = self
            .start_runner(&mut allocation, &profile, &reporter, &cancellation, &ip)
            .await?;
        let supervised = self
            .job
            .supervise(started, &allocation, &profile, &reporter, &cancellation)
            .await;
        // Warm retention runs while the guest is still up and before the Tart
        // child is dropped; it reports an outcome and never fails the
        // allocation.
        if let Some(vm) = &mut vm
            && supervised.is_ok()
        {
            self.ensure_retained(&allocation, &profile, vm, &ip, &reporter, &cancellation)
                .await;
        }
        // The guest this allocation cloned is handed to the pool before `run`
        // returns and `kill_on_drop` takes the VM with it. Parked, not claimed:
        // the manager decides whether to keep it, and cleanup drops whatever it
        // declined so nothing is left running with no supervisor.
        if let Some(vm) = vm
            && allocation.hot_lane.is_some()
            && supervised.is_ok()
        {
            self.hot.park(&allocation.vm_name, vm).await;
        }
        supervised
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        self.tart.host_machines().await
    }

    /// Probes with the bounded listing, so a missing or wedged `tart` binary
    /// reports Unavailable instead of the neutral default's silent Healthy.
    async fn runtime_status(&self) -> RuntimeStatus {
        match self.tart.list().await {
            Ok(_) => RuntimeStatus::tart(RuntimeHealth::Healthy, None),
            Err(error) => RuntimeStatus::tart(RuntimeHealth::Unavailable, Some(&error.to_string())),
        }
    }

    async fn base_fingerprint(&self, template: &VmName) -> Option<BaseFingerprint> {
        self.tart.fingerprint(template).await
    }

    /// The source is one of this profile's own declared names, so its disk is
    /// measured from the library the `tart` child itself reads.
    async fn base_storage_mb(&self, _profile: &ProfileName, source: &CloneSource) -> Option<u64> {
        self.tart.base_storage_mb(source.name.as_str()).await
    }

    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        // A removed profile leaves the record as the only authority, so live
        // configuration is refused here rather than through it.
        if request.reserved.contains(request.name.as_str()) {
            return Err(WorkerError::new(
                "refusing to delete a name live configuration claims",
            ));
        }
        match request.authorization {
            // A record names the clone, and so does the configured prefix on
            // its own: `delete_owned` refuses anything outside that boundary.
            ReapAuthorization::Record(_) | ReapAuthorization::Prefix => {
                // A wedged `tart` child would otherwise stall the reaper for
                // good; the budget is the same clock every teardown reads.
                tokio::time::timeout(
                    request.budget.timeout(),
                    self.tart.delete_owned(request.name),
                )
                .await
                .map_err(|_| WorkerError::new("Tart reap deletion exceeded its budget"))??;
                // The registration outlives the clone otherwise, and nothing
                // later rediscovers it. An orphan with no record leaves one
                // only its own registration expiry can reach.
                if let Some(allocation) = request.allocation
                    && let Err(error) = self.job.delete_runner(allocation).await
                {
                    tracing::warn!(%error, "reaped VM left its Forgejo registration");
                }
                Ok(())
            }
            ReapAuthorization::Staging(_) | ReapAuthorization::Image(_) => {
                if request.profile.is_none() && request.record.is_none() {
                    return Err(WorkerError::new(
                        "image deletion needs a profile or a record that claims the name",
                    ));
                }
                tokio::time::timeout(
                    request.budget.timeout(),
                    self.tart
                        .delete_image(request.name, request.profile, request.record),
                )
                .await
                .map_err(|_| WorkerError::new("Tart image deletion exceeded its budget"))?
            }
        }
    }

    /// Clones the retained `<warm>.previous` generation back over the live
    /// name. The presence check lives here rather than in the manager, which
    /// has no business computing a name-shaped fact for a backend.
    async fn ensure_warm_restored(
        &self,
        profile: &Profile,
        record: &WarmImageRecord,
    ) -> Result<(), WorkerError> {
        let previous = record
            .previous_name()
            .map_err(|error| WorkerError::new(error.to_string()))?;
        if self.tart.is_absent_image(&previous).await {
            return Err(WorkerError::new(
                "no warm image survives this profile; allocations boot cold",
            ));
        }
        // Runs during startup recovery, where a wedged `tart clone` would
        // otherwise hang the daemon before it ever binds the listener.
        tokio::time::timeout(
            Duration::from_secs(profile.cleanup_timeout_seconds),
            self.tart
                .clone_image(&previous, &record.warm_template, profile, None),
        )
        .await
        .map_err(|_| WorkerError::new("warm image restore exceeded its timeout"))?
    }

    async fn ensure_hot_retained(&self, request: HotRetainRequest<'_>) -> Result<(), WorkerError> {
        self.retain_hot(request).await
    }

    async fn ensure_hot_reset(
        &self,
        guest: &flanforge_core::HotGuest,
        reset: HotReset,
    ) -> Result<(), WorkerError> {
        self.reset_hot(guest, reset).await
    }

    async fn is_hot_live(&self, guest: &flanforge_core::HotGuest) -> bool {
        self.is_hot_alive(guest).await
    }

    async fn ensure_hot_evicted(
        &self,
        guest: &flanforge_core::HotGuest,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.evict_hot(guest, budget).await
    }

    async fn cleanup(
        &self,
        allocation: Allocation,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        tracing::info!(allocation_id = %allocation.id, "allocation cleanup started");
        // A machine the pool kept is not this allocation's to destroy, and a
        // reused one was never this allocation's at all. Anything the pool
        // parked but declined is dropped here, which kills its VM — so a
        // machine is never left running with nothing supervising it.
        let is_pooled = if allocation.origin.is_hot_reuse() {
            true
        } else {
            self.hot.release_unclaimed(&allocation.vm_name).await;
            self.hot.is_claimed(&allocation.vm_name).await
        };
        let vm_result = if is_pooled {
            tracing::info!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "the guest is the pool's; cleanup deletes the runner registration only");
            Ok(())
        } else {
            // The steps run back to back, so the second is measured against
            // what a shutdown grace still allows once the first has spent from
            // it.
            tokio::time::timeout(budget.timeout(), self.tart.remove_owned(&allocation))
                .await
                .map_err(|_| WorkerError::new("Tart cleanup exceeded its timeout"))?
        };
        let runner_result = tokio::time::timeout(budget.timeout(), async {
            self.job.delete_runner(&allocation).await
        })
        .await
        .map_err(|_| WorkerError::new("Forgejo cleanup exceeded its timeout"))?;
        match (vm_result, runner_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(vm), Ok(()) | Err(_)) => Err(vm),
            (Ok(()), Err(runner)) => Err(runner),
        }
    }
}
