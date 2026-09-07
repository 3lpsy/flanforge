use flanforge_manager::WorkerError;
use tokio::process::Child;

use crate::channel::{GuestExit, GuestProcess};

/// One SSH client process. `kill_on_drop` plus the remote script's own trap
/// means losing this handle ends the guest-side job too.
#[derive(Debug)]
pub(crate) struct SshProcess(Child);

impl SshProcess {
    pub(super) const fn new(child: Child) -> Self {
        Self(child)
    }
}

#[async_trait::async_trait]
impl GuestProcess for SshProcess {
    async fn try_exit(&mut self) -> Result<Option<GuestExit>, WorkerError> {
        self.0
            .try_wait()
            .map(|status| status.map(GuestExit::from))
            .map_err(|_| WorkerError::new("cannot inspect guest runner"))
    }

    async fn ensure_stopped(&mut self) {
        let _ = self.0.kill().await;
    }

    /// The client process is observed directly, so there is no probe schedule
    /// to shorten.
    fn probe_now(&mut self) {}
}

/// Lets a backend's supervision tests drive the real process type with a child
/// they control, instead of reimplementing it.
#[cfg(feature = "test-support")]
#[must_use]
pub fn ssh_process_for_test(child: Child) -> Box<dyn GuestProcess> {
    Box::new(SshProcess::new(child))
}
