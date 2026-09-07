use flanforge_core::Allocation;
use flanforge_forgejo::ForgejoClient;
use flanforge_manager::WorkerError;

/// The Forgejo half of one allocation's runner. It needs no guest channel, so a
/// worker can own it before any guest exists and keep using it through cleanup.
#[derive(Clone, Debug)]
pub struct RunnerRegistration {
    forgejo: ForgejoClient,
}

impl RunnerRegistration {
    #[must_use]
    pub const fn new(forgejo: ForgejoClient) -> Self {
        Self { forgejo }
    }

    /// Deletes the exact runner registration owned by one allocation.
    ///
    /// # Errors
    ///
    /// Returns an error when Forgejo refuses or cannot complete the cleanup.
    pub async fn delete_runner(&self, allocation: &Allocation) -> Result<(), WorkerError> {
        if let Some(runner_id) = allocation.runner_id {
            self.forgejo
                .delete_runner(&allocation.request.repository, runner_id)
                .await
        } else {
            self.forgejo
                .delete_runners_named(
                    &allocation.request.repository,
                    &format!("flanforged-{}", allocation.id),
                )
                .await
        }
        .map_err(|error| WorkerError::new(error.to_string()))
    }

    pub(crate) const fn client(&self) -> &ForgejoClient {
        &self.forgejo
    }
}
