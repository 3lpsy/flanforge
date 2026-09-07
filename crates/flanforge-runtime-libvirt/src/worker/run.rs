use std::time::Duration;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationState, BaseFingerprint, CloneSource, HotGuest, Profile, ProfileName,
    VmName, WarmImageRecord,
};
use flanforge_manager::{
    AllocationReporter, AllocationWorker, CleanupBudget, HostMachine, HotReset, HotRetainRequest,
    ImageSweep, ReapAuthorization, ReapRequest, RetiredImage, WarmAvailability, WorkerError,
};
use flanforge_wire::{RuntimeCapabilities, RuntimeHealth, RuntimeStatus};
use tokio_util::sync::CancellationToken;

use crate::image::load_published;

use super::{
    LibvirtWorker,
    cleanup::{GuestCleanup, finalize_local_state},
    reap::find_allocation,
};

#[async_trait]
impl AllocationWorker for LibvirtWorker {
    async fn run(
        &self,
        mut allocation: Allocation,
        profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        // A reused machine is already booted and provisioned, so the clone, the
        // boot, and the readiness gate are all skipped and `Preparing` never
        // happens. The manager proved it live before this was entered.
        let pooled = allocation.origin.hot_vm_name().cloned();
        let prepared = if let Some(name) = &pooled {
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(profile.boot_timeout_seconds);
            self.bind_hot(name, &profile.template, &cancellation, deadline)
                .await?
        } else {
            reporter
                .transition(AllocationState::Preparing)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
            self.prepare(&mut allocation, &profile, &reporter, &cancellation)
                .await?
        };
        reporter
            .transition(AllocationState::Registering)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let started = prepared
            .job
            .start_runner(
                &mut allocation,
                &profile,
                &reporter,
                &cancellation,
                &prepared.session,
            )
            .await?;
        let supervised = prepared
            .job
            .supervise(started, &allocation, &profile, &reporter, &cancellation)
            .await;
        // Retention runs while the domain is still defined and before cleanup
        // deletes the overlay; it reports an outcome and never fails the
        // allocation. A reused machine's overlay is the pool's rather than this
        // allocation's, so it is never captured.
        if pooled.is_none() && supervised.is_ok() {
            self.ensure_retained(&allocation, &profile, &prepared, &reporter)
                .await;
        }
        supervised
    }

    async fn cleanup(
        &self,
        allocation: Allocation,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        // A machine the pool kept is not this allocation's to destroy, and a
        // reused one was never this allocation's at all. libvirt has no park
        // step — a domain needs no supervising process — so the claim set only
        // ever holds machines the pool took, and there is nothing to withdraw
        // here the way Tart withdraws a guest it parked but declined.
        if allocation.origin.is_hot_reuse() || self.hot.is_claimed(&allocation.vm_name).await {
            tracing::info!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "the guest is the pool's; cleanup deletes the runner registration only");
            // The ownership manifest and its known_hosts are the pool machine's
            // own authority — eviction and every later claim read them — so no
            // tombstone is written and no authority file is removed.
            return tokio::time::timeout(
                budget.timeout(),
                self.registration.delete_runner(&allocation),
            )
            .await
            .map_err(|_| WorkerError::new("Forgejo cleanup exceeded its timeout"))?;
        }
        let slack = Duration::from_secs(flanforge_core::LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS);
        // The actor reserves parent slack inside `timeout`, so this outer guard
        // must outlast it. Firing together drops the helper future and
        // `kill_on_drop` SIGKILLs it mid-mutation, which is exactly what
        // RUN-738's graceful path exists to avoid. A shutdown grace caps the
        // guard, so the slack comes out of the guest budget rather than running
        // past the deadline.
        let guard = budget.limit(budget.allowance().saturating_add(slack));
        let timeout = guard.saturating_sub(slack);
        let (guest, runner) = tokio::join!(
            tokio::time::timeout(guard, self.cleanup_guest(&allocation, timeout)),
            tokio::time::timeout(
                budget.timeout(),
                self.registration.delete_runner(&allocation)
            ),
        );
        let guest = guest
            .map_err(|_| WorkerError::new("libvirt cleanup exceeded its timeout"))
            .and_then(|outcome| outcome);
        let runner = runner
            .map_err(|_| WorkerError::new("Forgejo cleanup exceeded its timeout"))
            .and_then(|outcome| outcome);
        finish_cleanup(&self.state_dir, allocation.id.into_uuid(), guest, runner).await
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        self.actor
            .inventory(Duration::from_secs(15))
            .await
            .map_err(Into::into)
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::libvirt()
    }

    /// Answered from the durable pointer, never from a name: this backend's
    /// listing reports domains, and a warm generation is a volume.
    async fn warm_availability(
        &self,
        _profile: &Profile,
        record: &WarmImageRecord,
        _listing: &[HostMachine],
    ) -> Result<WarmAvailability, WorkerError> {
        self.warm_pointer_availability(record).await
    }

    /// No image is name-addressed here, so nothing can sit under a declared
    /// name that the daemon never recorded.
    async fn is_unclaimed_image(
        &self,
        _name: &VmName,
        _listing: &[HostMachine],
    ) -> Result<bool, WorkerError> {
        Ok(false)
    }

    async fn warm_retained(&self, profile: &ProfileName) -> Option<u32> {
        self.retained_generations(profile).await
    }

    async fn ensure_recovered(&self) -> Result<(), WorkerError> {
        self.ensure_warm_recovered().await
    }

    async fn ensure_warm_restored(
        &self,
        _profile: &Profile,
        record: &WarmImageRecord,
    ) -> Result<(), WorkerError> {
        self.ensure_pointer_restored(record).await
    }

    async fn sweep_images(
        &self,
        request: ImageSweep<'_>,
    ) -> Result<Vec<RetiredImage>, WorkerError> {
        self.ensure_superseded_retired(request).await
    }

    async fn runtime_status(&self) -> RuntimeStatus {
        match self.actor.probe(Duration::from_secs(10)).await {
            Ok(()) => RuntimeStatus::libvirt(
                RuntimeHealth::Degraded,
                Some("experimental backend; real-host qualification is incomplete"),
            ),
            Err(error) => {
                RuntimeStatus::libvirt(RuntimeHealth::Unavailable, Some(&error.to_string()))
            }
        }
    }

    async fn base_fingerprint(&self, template: &VmName) -> Option<BaseFingerprint> {
        let publication = load_published(&self.image_manifest_dir, template).ok()?;
        BaseFingerprint::new(publication.manifest().image_sha256()).ok()
    }

    async fn base_storage_mb(&self, profile: &ProfileName, source: &CloneSource) -> Option<u64> {
        self.source_storage_mb(profile, source).await
    }

    /// Collects a guest whose cleanup never finished. Durable evidence
    /// authorizes this: the allocation record, or — once that record has aged
    /// out — the ownership manifest that still claims the domain. A name live
    /// configuration claims is refused outright.
    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        if request.reserved.contains(request.name.as_str()) {
            return Err(WorkerError::new(
                "refusing to delete a name live configuration claims",
            ));
        }
        let allocation_id =
            reapable_allocation(&self.state_dir, request.authorization, request.name).await?;
        self.ensure_reaped(
            allocation_id,
            request.name,
            request.budget,
            request.allocation,
        )
        .await
    }

    async fn ensure_hot_retained(&self, request: HotRetainRequest<'_>) -> Result<(), WorkerError> {
        self.retain_hot(request).await
    }

    async fn ensure_hot_reset(&self, guest: &HotGuest, reset: HotReset) -> Result<(), WorkerError> {
        self.reset_hot(guest, reset).await
    }

    async fn is_hot_live(&self, guest: &HotGuest) -> bool {
        self.is_hot_alive(guest).await
    }

    async fn ensure_hot_evicted(
        &self,
        guest: &HotGuest,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.evict_hot(guest, budget).await
    }
}

/// The allocation a deletion may act on. A record names one directly; a
/// prefix-authorized orphan is resolved from the durable manifest that still
/// claims the domain, which is the ownership evidence a name cannot forge.
async fn reapable_allocation(
    state_dir: &std::path::Path,
    authorization: &ReapAuthorization,
    name: &VmName,
) -> Result<uuid::Uuid, WorkerError> {
    match authorization {
        ReapAuthorization::Record(allocation_id) => Ok(allocation_id.into_uuid()),
        ReapAuthorization::Prefix => find_allocation(state_dir, name).await,
        ReapAuthorization::Staging(_) | ReapAuthorization::Image(_) => Err(WorkerError::new(
            "libvirt retires images on the sweep, never through a deletion",
        )),
    }
}

async fn finish_cleanup(
    state_dir: &std::path::Path,
    allocation_id: uuid::Uuid,
    guest: Result<GuestCleanup, WorkerError>,
    runner: Result<(), WorkerError>,
) -> Result<(), WorkerError> {
    let guest = guest?;
    if matches!(guest, GuestCleanup::Tombstoned) {
        return Ok(());
    }
    runner?;
    finalize_local_state(state_dir, allocation_id, guest).await
}

#[cfg(test)]
mod tests;
