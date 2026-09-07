use std::time::Duration;

use flanforge_core::{Allocation, AllocationMode, AllocationState, CloneKind, Profile};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::{AllocationReporter, WorkerError};

use flanforge_runtime::{GuestSession, StartedRunner};

use super::FlanForgeWorker;

impl FlanForgeWorker {
    pub(super) async fn prepare_vm(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
        boot_timeout: Duration,
    ) -> Result<(Child, String), WorkerError> {
        tracing::info!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "preparing Tart guest");
        self.clone_current_source(allocation, profile, reporter, cancellation, deadline)
            .await?;
        allocation.set_vm_created();
        if let Err(error) = reporter.set_vm_created().await {
            let _ = tokio::time::timeout(
                Duration::from_secs(profile.cleanup_timeout_seconds),
                self.tart.remove_owned(allocation),
            )
            .await;
            return Err(WorkerError::new(error.to_string()));
        }
        Self::phase(
            cancellation,
            deadline,
            "Tart configuration exceeded its timeout",
            self.tart.configure(allocation, profile),
        )
        .await?;
        reporter
            .transition(AllocationState::Booting)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let mut vm = self.tart.start(allocation, profile)?;
        // A `tart run` that dies at launch (softnet refused, port in use)
        // would otherwise burn the whole boot budget polling for an IP the
        // dead child can never obtain, holding the slot the entire time.
        let ip = tokio::select! {
            status = vm.wait() => {
                let exit_code = status.ok().and_then(|status| status.code());
                tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, exit_code, "Tart VM exited during boot");
                return Err(WorkerError::new("Tart VM exited before obtaining an IP address"));
            }
            ip = Self::phase(
                cancellation,
                deadline,
                "Tart VM boot exceeded its timeout",
                self.tart.wait_for_ip(allocation, boot_timeout),
            ) => ip?,
        };
        let session = GuestSession::configured(&ip)?;
        Self::phase(
            cancellation,
            deadline,
            "guest readiness exceeded its timeout",
            self.job.ensure_ready(&session, boot_timeout),
        )
        .await?;
        tracing::info!(allocation_id = %allocation.id, "Tart guest boot completed");
        Ok((vm, ip))
    }

    pub(crate) async fn clone_current_source(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkerError> {
        let (source, warm_generation) = loop {
            let planned = Self::phase(
                cancellation,
                deadline,
                "Tart source resolution exceeded its timeout",
                async {
                    reporter
                        .resolve_clone_source(profile)
                        .await
                        .map_err(|error| WorkerError::new(error.to_string()))
                },
            )
            .await?;
            allocation.set_clone_source(planned.source, planned.warm_generation);
            Self::phase(
                cancellation,
                deadline,
                "Tart preparation exceeded its timeout",
                self.tart.ensure_can_clone(allocation, profile, deadline),
            )
            .await?;

            let image_guard = if allocation.mode == AllocationMode::Warm {
                Some(
                    Self::phase(
                        cancellation,
                        deadline,
                        "Tart image lock exceeded the preparation timeout",
                        async { Ok(self.lock_profile_image(&allocation.request.profile).await) },
                    )
                    .await?,
                )
            } else {
                None
            };
            let selected = Self::phase(
                cancellation,
                deadline,
                "Tart clone-boundary source resolution exceeded its timeout",
                async {
                    reporter
                        .resolve_clone_source(profile)
                        .await
                        .map_err(|error| WorkerError::new(error.to_string()))
                },
            )
            .await?;
            allocation.set_clone_source(selected.source, selected.warm_generation);
            let mut source = match Self::phase(
                cancellation,
                deadline,
                "Tart clone-boundary check exceeded its timeout",
                self.tart.ensure_can_clone_now(allocation, profile),
            )
            .await
            {
                Ok(source) => source,
                Err(error) if error.is_capacity() => {
                    drop(image_guard);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let generation = (source.kind == CloneKind::Warm)
                .then_some(selected.warm_generation)
                .flatten();
            if source.kind == CloneKind::Template {
                source.base_fingerprint = self.tart.fingerprint(&source.name).await;
            }
            Self::phase(
                cancellation,
                deadline,
                "Tart clone exceeded its timeout",
                self.tart.clone(allocation, &source.name),
            )
            .await?;
            reporter
                .set_clone_source(source.clone(), generation)
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
            drop(image_guard);
            break (source, generation);
        };
        allocation.set_clone_source(source, warm_generation);
        Ok(())
    }

    pub(super) async fn start_runner(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        ip: &str,
    ) -> Result<StartedRunner, WorkerError> {
        let session = GuestSession::configured(ip)?;
        self.job
            .start_runner(allocation, profile, reporter, cancellation, &session)
            .await
    }
}
