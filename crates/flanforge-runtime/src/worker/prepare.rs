use std::time::Duration;

use flanforge_core::{Allocation, AllocationState, Profile};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::{AllocationReporter, WorkerError};

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
        let mut source = Self::phase(
            cancellation,
            deadline,
            "Tart preparation exceeded its timeout",
            self.tart.ensure_can_clone(allocation, profile),
        )
        .await?;
        // Captured here rather than at create time: this is the base the clone
        // was actually taken from.
        source.base_fingerprint = self.tart.fingerprint(&profile.template).await;
        if allocation.source.as_ref() != Some(&source) {
            allocation.set_source(source.clone());
            reporter
                .set_source(source.clone())
                .await
                .map_err(|error| WorkerError::new(error.to_string()))?;
        }
        Self::phase(
            cancellation,
            deadline,
            "Tart clone exceeded its timeout",
            self.tart.clone(allocation, &source.name),
        )
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
        let vm = self.tart.start(allocation, profile)?;
        let ip = Self::phase(
            cancellation,
            deadline,
            "Tart VM boot exceeded its timeout",
            self.tart.wait_for_ip(allocation, boot_timeout),
        )
        .await?;
        Self::phase(
            cancellation,
            deadline,
            "guest SSH readiness exceeded its timeout",
            self.guest.wait_ready(&ip, boot_timeout),
        )
        .await?;
        Self::phase(
            cancellation,
            deadline,
            "guest Tailscale bootstrap exceeded its timeout",
            self.guest.ensure_tailscale_connected(&ip),
        )
        .await?;
        tracing::info!(allocation_id = %allocation.id, "Tart guest boot completed");
        Ok((vm, ip))
    }

    pub(super) async fn start_runner(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        ip: &str,
    ) -> Result<Child, WorkerError> {
        // Registration and staging get their own budget: a slow but successful
        // boot must not leave them with an already-consumed deadline.
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.boot_timeout_seconds);
        let runner_name = format!("flanforged-{}", allocation.id);
        tracing::info!(allocation_id = %allocation.id, "registering ephemeral Forgejo runner");
        let credentials = Self::phase(
            cancellation,
            deadline,
            "runner registration exceeded its timeout",
            async {
                self.forgejo
                    .create_runner(&allocation.request.repository, &runner_name)
                    .await
                    .map_err(|error| WorkerError::new(error.to_string()))
            },
        )
        .await?;
        if let Err(error) = reporter.set_runner_id(credentials.id).await {
            let _ = self
                .forgejo
                .delete_runner(&allocation.request.repository, credentials.id)
                .await;
            return Err(WorkerError::new(error.to_string()));
        }
        allocation.set_runner_id(credentials.id);
        Self::phase(
            cancellation,
            deadline,
            "guest runner staging exceeded its timeout",
            self.guest.stage_runner(ip, self.tart.runner_host_path()),
        )
        .await?;
        reporter
            .transition(AllocationState::WaitingForJob)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        let job_deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.idle_timeout_seconds);
        let handle = self
            .wait_for_job_handle(allocation, profile, cancellation, job_deadline)
            .await?;
        tracing::info!(allocation_id = %allocation.id, "authorized Forgejo job is ready");
        // A job bound at the very end of the idle window still needs time to
        // start its runner, so the spawn gets a fresh short budget.
        let spawn_deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.cleanup_timeout_seconds);
        Self::phase(
            cancellation,
            spawn_deadline,
            "guest runner start exceeded its timeout",
            self.guest.spawn_runner(
                ip,
                self.forgejo.server_url().as_str(),
                &credentials,
                allocation.runner_label.as_str(),
                &handle,
            ),
        )
        .await
    }
}
