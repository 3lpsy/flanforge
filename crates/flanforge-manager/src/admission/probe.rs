use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::{AllocationManager, HostMachine, WorkerError};

/// How often a failing probe is allowed to warn, so a broken host listing does
/// not drown the log at request rate.
const WARNING_INTERVAL: Duration = Duration::from_mins(1);

/// The host listing, cached for `runtime.poll_seconds`.
#[derive(Debug, Default)]
pub(crate) struct MachineProbe {
    cached: Option<(Instant, Arc<Vec<HostMachine>>)>,
    warned_at: Option<Instant>,
}

impl AllocationManager {
    /// Lists the host, reusing a listing younger than one poll interval. Taken
    /// before the entries lock so no subprocess runs under that mutex.
    ///
    /// # Errors
    ///
    /// Returns the worker's error when the host cannot be listed; a stale
    /// listing is never served in its place.
    pub(crate) async fn host_machines(&self) -> Result<Arc<Vec<HostMachine>>, WorkerError> {
        let ttl = Duration::from_secs(self.inner.config.current().runtime.poll_seconds);
        let mut probe = self.inner.probe.lock().await;
        if let Some((taken_at, machines)) = &probe.cached
            && taken_at.elapsed() < ttl
        {
            return Ok(Arc::clone(machines));
        }
        match self.inner.worker.machines().await {
            Ok(machines) => {
                let machines = Arc::new(machines);
                probe.cached = Some((Instant::now(), Arc::clone(&machines)));
                Ok(machines)
            }
            Err(error) => {
                // Capacity we cannot see is capacity we do not have.
                if probe
                    .warned_at
                    .is_none_or(|at| at.elapsed() >= WARNING_INTERVAL)
                {
                    probe.warned_at = Some(Instant::now());
                    tracing::warn!(%error, "cannot list the host; admission answers busy");
                }
                Err(error)
            }
        }
    }
}
