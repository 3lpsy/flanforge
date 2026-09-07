use flanforge_manager::WorkerError;
use flanforge_runtime::retention_marker_script;

use super::super::worker::FlanForgeWorker;

impl FlanForgeWorker {
    pub(crate) async fn ensure_retention_marker(&self, address: &str) -> Result<(), WorkerError> {
        let result = self
            .job
            .run_daemon_script(address, &retention_marker_script())
            .await;
        match result {
            Ok(()) => {
                tracing::info!("workflow completion marker verified");
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, "workflow completion marker did not verify");
                Err(WorkerError::new(format!(
                    "workflow completion marker did not verify: {error}"
                )))
            }
        }
    }
}
