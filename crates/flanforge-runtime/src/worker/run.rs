use std::{future::Future, time::Duration};

use async_trait::async_trait;
use flanforge_core::{Allocation, AllocationState, BaseFingerprint, Config, Profile, VmName};
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::ForgejoClient;
use flanforge_manager::{
    AllocationReporter, AllocationWorker, HostMachine, ReapAuthorization, ReapRequest, WorkerError,
};

use super::super::{guest::GuestControl, tart::TartClient};

#[derive(Clone, Debug)]
pub struct FlanForgeWorker {
    pub(crate) tart: TartClient,
    pub(crate) guest: GuestControl,
    pub(crate) forgejo: ForgejoClient,
    pub(crate) poll: Duration,
}

impl FlanForgeWorker {
    #[must_use]
    pub fn new(config: &Config, forgejo: ForgejoClient) -> Self {
        Self {
            tart: TartClient::new(config.runtime.clone()),
            guest: GuestControl::new(
                config.guest.clone(),
                config.tailscale.clone(),
                config.runtime.ssh_path.clone(),
                config.runtime.scp_path.clone(),
            ),
            forgejo,
            poll: Duration::from_secs(config.runtime.poll_seconds),
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
        tokio::select! {
            () = cancellation.cancelled() => Err(WorkerError::new("allocation was cancelled")),
            result = tokio::time::timeout_at(deadline, future) => {
                result.map_err(|_| WorkerError::new(timeout_message))?
            }
        }
    }
}

#[async_trait]
impl AllocationWorker for FlanForgeWorker {
    async fn run(
        &self,
        mut allocation: Allocation,
        profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        tracing::info!(allocation_id = %allocation.id, profile = %allocation.request.profile, "allocation worker started");
        reporter
            .transition(AllocationState::Preparing)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let boot_timeout = Duration::from_secs(profile.boot_timeout_seconds);
        let boot_deadline = tokio::time::Instant::now() + boot_timeout;
        let (mut vm, ip) = self
            .prepare_vm(
                &mut allocation,
                &profile,
                &reporter,
                &cancellation,
                boot_deadline,
                boot_timeout,
            )
            .await?;
        reporter
            .transition(AllocationState::Registering)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let child = self
            .start_runner(&mut allocation, &profile, &reporter, &cancellation, &ip)
            .await?;
        let supervised = self
            .supervise(child, &allocation, &profile, &reporter, &cancellation)
            .await;
        // Retention runs while the guest is still up and before the Tart child
        // is dropped; it reports an outcome and never fails the allocation.
        if supervised.is_ok() {
            self.ensure_retained(
                &allocation,
                &profile,
                &mut vm,
                &ip,
                &reporter,
                &cancellation,
            )
            .await;
        }
        supervised
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        self.tart.host_machines().await
    }

    async fn base_fingerprint(&self, template: &VmName) -> Option<BaseFingerprint> {
        self.tart.fingerprint(template).await
    }

    async fn delete_vm(&self, request: ReapRequest<'_>) -> Result<(), WorkerError> {
        match request.authorization {
            ReapAuthorization::Record(_) => self.tart.delete_owned(request.name).await,
            ReapAuthorization::Staging(_) | ReapAuthorization::Image(_) => {
                // A removed profile leaves the record as the only authority, so
                // live configuration is refused here rather than through it.
                if request.reserved.contains(request.name.as_str()) {
                    return Err(WorkerError::new(
                        "refusing to delete a name live configuration claims",
                    ));
                }
                if request.profile.is_none() && request.record.is_none() {
                    return Err(WorkerError::new(
                        "image deletion needs a profile or a record that claims the name",
                    ));
                }
                self.tart
                    .delete_image(request.name, request.profile, request.record)
                    .await
            }
            // A prefix-shaped name with no record is reported, never deleted.
            ReapAuthorization::Prefix => Err(WorkerError::new(
                "refusing to delete a VM no record authorizes",
            )),
        }
    }

    async fn clone_image(
        &self,
        source: &VmName,
        destination: &VmName,
        profile: &Profile,
    ) -> Result<(), WorkerError> {
        self.tart
            .clone_image(source, destination, profile, None)
            .await
    }

    async fn cleanup(&self, allocation: Allocation, profile: Profile) -> Result<(), WorkerError> {
        tracing::info!(allocation_id = %allocation.id, "allocation cleanup started");
        let timeout = Duration::from_secs(profile.cleanup_timeout_seconds);
        let vm_result = tokio::time::timeout(timeout, self.tart.remove_owned(&allocation))
            .await
            .map_err(|_| WorkerError::new("Tart cleanup exceeded its timeout"))?;
        let runner_result = tokio::time::timeout(timeout, async {
            if let Some(runner_id) = allocation.runner_id {
                self.forgejo
                    .delete_runner(&allocation.request.repository, runner_id)
                    .await
                    .map_err(|error| WorkerError::new(error.to_string()))
            } else {
                self.forgejo
                    .delete_runners_named(
                        &allocation.request.repository,
                        &format!("flanforged-{}", allocation.id),
                    )
                    .await
                    .map_err(|error| WorkerError::new(error.to_string()))
            }
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
