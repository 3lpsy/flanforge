use std::time::Duration;

use flanforge_core::{Allocation, GuestSize, Profile};
use flanforge_manager::{AllocationReporter, WorkerError};
use tokio_util::sync::CancellationToken;

use flanforge_core::GuestChannelKind;

use crate::{
    agent::GUEST_CONTRACT_VERSION,
    image::{GuestContract, load_published},
};

use super::{LibvirtWorker, model::PreparedGuest};

mod bind;
mod boot;
mod source;

use source::effective_storage_bytes;

impl LibvirtWorker {
    pub(super) async fn prepare(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) -> Result<PreparedGuest, WorkerError> {
        let timeout = Duration::from_secs(profile.boot_timeout_seconds);
        let deadline = tokio::time::Instant::now() + timeout;
        // Read before anything is created: a base that can never satisfy the
        // selected channel costs zero seconds instead of a boot timeout.
        let contract = GuestContract::read(
            &load_published_async(self.image_manifest_dir.clone(), profile.template.clone())
                .await?,
        );
        contract.ensure_channel_supported(self.channel, &self.guest_user)?;
        let source = self.resolve_source(allocation, profile, reporter).await?;
        let size = allocation.size.unwrap_or(GuestSize {
            cpu_count: profile.cpu_count,
            memory_mb: profile.memory_mb,
            storage_mb: profile.storage_mb,
        });
        let storage_bytes =
            effective_storage_bytes(allocation, size.storage_mb, source.virtual_bytes())?;
        let manifest = self
            .boot_domain(
                allocation,
                size,
                source,
                storage_bytes,
                reporter,
                cancellation,
                deadline,
            )
            .await?;
        ensure_not_cancelled(cancellation, "after domain start")?;
        // The agent channel exists precisely because the daemon may have no
        // route to the guest, so it never waits for one.
        let address = match self.channel {
            GuestChannelKind::Ssh => Some(
                self.wait_for_address(&manifest, cancellation, deadline)
                    .await?,
            ),
            GuestChannelKind::Agent => None,
        };
        let prepared = self.bind_guest(&manifest, address, &contract)?;
        flanforge_runtime::GuestJob::phase(
            cancellation,
            deadline,
            "libvirt guest readiness exceeded its timeout",
            prepared
                .job
                .ensure_channel_ready(&prepared.session, remaining(deadline)?),
        )
        .await?;
        self.ensure_provisioned(&prepared, &contract, cancellation, deadline)
            .await?;
        flanforge_runtime::GuestJob::phase(
            cancellation,
            deadline,
            "libvirt guest Tailscale bootstrap exceeded its timeout",
            prepared.job.ensure_tailscale_ready(&prepared.session),
        )
        .await?;
        Ok(prepared)
    }

    /// Agent liveness and an SSH login both prove reachability, not that
    /// first-boot provisioning finished. The baked helper answers that, over
    /// whichever channel is bound — so SSH gets the same named causes.
    ///
    /// A base built before the helper existed has nothing to ask, so it keeps
    /// today's behaviour rather than failing every allocation.
    async fn ensure_provisioned(
        &self,
        prepared: &PreparedGuest,
        contract: &GuestContract,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkerError> {
        if !contract.is_readiness_gate_baked() {
            tracing::debug!("the published base bakes no readiness helper; skipping the gate");
            return Ok(());
        }
        flanforge_runtime::GuestJob::phase(
            cancellation,
            deadline,
            "libvirt guest provisioning exceeded its timeout",
            crate::agent::ensure_provisioned(
                prepared.job.channel(),
                &prepared.session,
                &self.guest_user,
                GUEST_CONTRACT_VERSION,
                deadline,
                self.poll,
            ),
        )
        .await
    }
}

fn ensure_not_cancelled(
    cancellation: &CancellationToken,
    boundary: &'static str,
) -> Result<(), WorkerError> {
    if cancellation.is_cancelled() {
        Err(WorkerError::new(format!(
            "allocation was cancelled {boundary}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;

pub(super) async fn load_published_async(
    directory: std::path::PathBuf,
    logical: flanforge_core::VmName,
) -> Result<flanforge_libvirt_wire::PublishedBase, WorkerError> {
    tokio::task::spawn_blocking(move || load_published(&directory, &logical))
        .await
        .map_err(|_| WorkerError::new("published-base task failed"))?
        .map_err(Into::into)
}

fn remaining(deadline: tokio::time::Instant) -> Result<Duration, WorkerError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| WorkerError::new("libvirt preparation exceeded its timeout"))
}
