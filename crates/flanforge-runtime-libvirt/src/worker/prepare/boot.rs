use std::time::Duration;

use flanforge_core::{Allocation, AllocationState, GuestSize};
use flanforge_libvirt_wire::{HelperFailureCode, OwnershipManifest, VolumePointer};
use flanforge_manager::{AllocationReporter, WorkerError};
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeError,
    actor::{CreateRequest, DefineRequest},
    checkpoint::{allocation_path as checkpoint_path, remove as remove_checkpoint},
    manifest::{
        allocation_dir, create_private_directory, intent, path, save_async, write_known_hosts,
    },
    seed::build_seed,
};

use super::super::LibvirtWorker;
use super::{ensure_not_cancelled, remaining};

impl LibvirtWorker {
    /// Seeds, creates, defines, and starts one allocation's domain. Every step
    /// is cancellable, and nothing durable is claimed before its record is.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn boot_domain(
        &self,
        allocation: &mut Allocation,
        size: GuestSize,
        source: VolumePointer,
        storage_bytes: u64,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<OwnershipManifest, WorkerError> {
        let allocation_id = allocation.id.to_string();
        let hostname = allocation.vm_name.to_string();
        let authorized_key = self.operator_public_key.clone();
        let privileged_key = self.privileged_public_key.clone();
        let guest_user = self.guest_user.clone();
        let privileged_user = self.current_config().guest.privileged_user.clone();
        let seed = tokio::task::spawn_blocking(move || {
            build_seed(
                &allocation_id,
                &hostname,
                authorized_key.as_deref(),
                &guest_user,
                &privileged_user,
                privileged_key.as_deref(),
            )
        })
        .await
        .map_err(|_| WorkerError::new("seed creation task failed"))??;
        let directory = allocation_dir(&self.state_dir, allocation.id.into_uuid());
        create_private_directory(&directory).await?;
        let manifest = intent(
            allocation,
            &self.instance,
            &self.state_dir,
            seed.host_key_alias,
        )?;
        write_known_hosts(manifest.known_hosts_file(), &seed.known_hosts).await?;
        let manifest_path = path(&self.state_dir, allocation.id.into_uuid());
        save_async(manifest.clone(), manifest_path.clone()).await?;
        ensure_not_cancelled(cancellation, "before guest resource creation")?;
        let manifest = self
            .actor
            .create(
                CreateRequest {
                    manifest,
                    source,
                    seed: seed.image,
                    storage_bytes,
                },
                remaining(deadline)?,
            )
            .await
            .map_err(WorkerError::from)?;
        self.persist_or_cleanup(
            &manifest,
            manifest_path,
            allocation.id.into_uuid(),
            deadline,
        )
        .await?;
        ensure_not_cancelled(cancellation, "after guest resource creation")?;
        self.actor
            .define(
                DefineRequest {
                    manifest: manifest.clone(),
                    cpu_count: size.cpu_count,
                    memory_mb: size.memory_mb,
                },
                remaining(deadline)?,
            )
            .await?;
        allocation.set_vm_created();
        reporter
            .set_vm_created()
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        ensure_not_cancelled(cancellation, "after domain definition")?;
        reporter
            .transition(AllocationState::Booting)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        self.actor
            .start(manifest.clone(), remaining(deadline)?)
            .await?;
        Ok(manifest)
    }

    /// A manifest that cannot be persisted leaves resources no durable record
    /// authorizes, so the guest is torn down before the error escapes.
    async fn persist_or_cleanup(
        &self,
        manifest: &flanforge_libvirt_wire::OwnershipManifest,
        manifest_path: std::path::PathBuf,
        allocation_id: uuid::Uuid,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkerError> {
        let Err(error) = save_async(manifest.clone(), manifest_path).await else {
            remove_checkpoint(&checkpoint_path(&self.state_dir, allocation_id))?;
            return Ok(());
        };
        let cleanup = self
            .actor
            .cleanup(
                manifest.clone(),
                remaining(deadline)
                    .unwrap_or_default()
                    .max(Duration::from_secs(
                        flanforge_core::LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS,
                    )),
            )
            .await;
        if cleanup.is_ok() {
            let _ = remove_checkpoint(&checkpoint_path(&self.state_dir, allocation_id));
        }
        Err(error.into())
    }

    pub(crate) async fn wait_for_address(
        &self,
        manifest: &flanforge_libvirt_wire::OwnershipManifest,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<std::net::IpAddr, WorkerError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(WorkerError::new("allocation was cancelled"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("libvirt guest did not obtain an address"));
            }
            match self
                .actor
                .address(
                    manifest.clone(),
                    remaining(deadline)?.min(Duration::from_secs(6)),
                )
                .await
            {
                Ok(Some(address)) => return Ok(address),
                // A libvirtd or virtqemud restart while the guest boots reports
                // Unavailable, and the next poll would have succeeded; the boot
                // deadline is what ends this loop, not one lost connection.
                Ok(None)
                | Err(
                    RuntimeError::Transient { .. }
                    | RuntimeError::Helper {
                        code: HelperFailureCode::Transient | HelperFailureCode::Unavailable,
                        ..
                    },
                ) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(self.poll).await;
        }
    }
}
