use std::{collections::HashMap, time::Duration};

use flanforge_core::{HotGuest, SimulatorReset, VmName};
use tokio::{process::Child, sync::Mutex};

use flanforge_manager::{CleanupBudget, HotReset, HotRetainRequest, WorkerError};
use flanforge_runtime::{
    GuestCapture, GuestCommand, GuestExit, GuestProgram, GuestSession, RECYCLE_CONTRACT_VERSION,
    RECYCLE_HELPER, RECYCLE_MAX_REPORT_BYTES, RecycleVerdict,
};

/// One machine the pool holds open, and the `tart run` child that keeps it
/// alive.
///
/// `tart run` is a child of `flanforged` with `kill_on_drop`, so the guest's
/// lifetime *is* this handle's lifetime. Moving it here is the whole of what
/// makes a Tart guest outlive the worker that built it — and it is also why a
/// daemon restart takes the pool with it, which the design accepts.
#[derive(Debug)]
struct PooledGuest {
    child: Child,
    /// False between the worker parking the child and the pool claiming it.
    /// Cleanup drops anything still false, so a machine the pool declined is
    /// never left running with nothing supervising it.
    is_claimed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct HotPool {
    guests: Mutex<HashMap<String, PooledGuest>>,
}

impl PooledGuest {
    /// Whether the `tart run` child is still there. Holding the handle is what
    /// keeps the VM alive, so a child that already exited is a machine that is
    /// already gone.
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl HotPool {
    /// Takes the guest's `tart run` child out of the worker's scope, before
    /// `run` returns and drops it. Unclaimed until the pool says otherwise.
    pub(crate) async fn park(&self, name: &VmName, child: Child) {
        self.guests.lock().await.insert(
            name.as_str().to_owned(),
            PooledGuest {
                child,
                is_claimed: false,
            },
        );
        tracing::debug!(vm_name = %name, "a finished allocation's guest is parked for the pool");
    }

    /// Marks a parked guest as the pool's. Returns false when nothing parked
    /// it, which means the machine is not this daemon's to keep.
    pub(crate) async fn claim(&self, name: &VmName) -> bool {
        match self.guests.lock().await.get_mut(name.as_str()) {
            Some(guest) => {
                guest.is_claimed = true;
                true
            }
            None => false,
        }
    }

    pub(crate) async fn is_claimed(&self, name: &VmName) -> bool {
        self.guests
            .lock()
            .await
            .get(name.as_str())
            .is_some_and(|guest| guest.is_claimed)
    }

    /// Whether the pool holds this machine *and* its process is still running.
    pub(crate) async fn is_holding(&self, name: &VmName) -> bool {
        self.guests
            .lock()
            .await
            .get_mut(name.as_str())
            .is_some_and(|guest| guest.is_claimed && guest.is_alive())
    }

    /// Drops a parked guest, which kills its VM. Returns whether one was held.
    pub(crate) async fn release(&self, name: &VmName) -> bool {
        let dropped = self.guests.lock().await.remove(name.as_str());
        if dropped.is_some() {
            tracing::debug!(vm_name = %name, "the pool released its hold on a guest");
        }
        dropped.is_some()
    }

    /// Drops a guest the pool never claimed. This is what stops a machine the
    /// pool declined from being left running with nothing supervising it.
    pub(crate) async fn release_unclaimed(&self, name: &VmName) -> bool {
        let mut guests = self.guests.lock().await;
        match guests.get(name.as_str()) {
            Some(guest) if !guest.is_claimed => {
                guests.remove(name.as_str());
                true
            }
            _ => false,
        }
    }
}

impl super::FlanForgeWorker {
    /// Takes a finished allocation's guest into the pool. The child was parked
    /// by `run`; this claims it and proves the machine came back clean.
    pub(super) async fn retain_hot(
        &self,
        request: HotRetainRequest<'_>,
    ) -> Result<(), WorkerError> {
        let name = &request.guest.vm_name;
        if !self.hot.claim(name).await {
            return Err(WorkerError::new(
                "this daemon does not hold the guest's process, so it cannot keep it",
            ));
        }
        if let Err(error) = self.reset_hot(request.guest, request.reset).await {
            // Dropping the hold kills the VM, which is the right outcome for a
            // machine that did not come back clean.
            self.hot.release(name).await;
            return Err(error);
        }
        Ok(())
    }

    /// Runs the recycle gate over a pool machine and reads its one verdict.
    ///
    /// The daemon runs one named command with a bounded timeout and reads one
    /// JSON line. Tart has no privileged channel, so this runs as the job
    /// account; the gate is written for that. It knows nothing about launchctl,
    /// simctl or dscl; the image does.
    pub(super) async fn reset_hot(
        &self,
        guest: &HotGuest,
        reset: HotReset,
    ) -> Result<(), WorkerError> {
        let budget = reset.budget;
        let address = self
            .tart
            .address_of(&guest.vm_name, budget.limit(Duration::from_secs(30)))
            .await?;
        let session = GuestSession::configured(&address)?;
        let arguments = [
            "--simulator-reset".to_owned(),
            simulator_reset(reset).to_owned(),
        ];
        let command = GuestCommand::new(
            GuestProgram::Program {
                path: RECYCLE_HELPER,
                arguments: &arguments,
            },
            None,
            GuestCapture::Bounded(RECYCLE_MAX_REPORT_BYTES),
        )?;
        let output =
            tokio::time::timeout(budget.timeout(), self.job.channel().run(&session, &command))
                .await
                .map_err(|_| WorkerError::new("the recycle gate exceeded its timeout"))?
                .map_err(|error| {
                    WorkerError::new(format!("the recycle gate could not run: {error}"))
                })?;
        let GuestExit::Code(code) = output.exit() else {
            return Err(WorkerError::new(
                "the recycle gate was terminated by a signal",
            ));
        };
        if output.is_truncated() {
            return Err(WorkerError::new("the recycle verdict is oversized"));
        }
        let verdict = RecycleVerdict::parse(output.stdout(), RECYCLE_CONTRACT_VERSION)
            .map_err(|error| WorkerError::new(format!("{error} (gate exit {code})")))?;
        if code == 0 && verdict.is_clean() {
            let (account, uid) = verdict.account();
            tracing::info!(
                vm_name = %guest.vm_name,
                free_mb = verdict.free_mb(),
                skew_seconds = verdict.skew_seconds(),
                account,
                uid,
                "the recycle gate passed"
            );
            return Ok(());
        }
        Err(WorkerError::new(verdict.cause()))
    }

    /// A bounded liveness probe: the pool still holds the process, and the
    /// host still reports the machine running.
    pub(super) async fn is_hot_alive(&self, guest: &HotGuest) -> bool {
        self.hot.is_holding(&guest.vm_name).await && self.tart.is_running(&guest.vm_name).await
    }

    /// Destroys a pool machine. Dropping the hold kills the `tart run` child;
    /// the delete then collects the stopped VM's disk.
    pub(super) async fn evict_hot(
        &self,
        guest: &HotGuest,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.tart.ensure_owned_name(&guest.vm_name)?;
        self.hot.release(&guest.vm_name).await;
        tokio::time::timeout(
            budget.timeout(),
            self.tart.remove_named(&guest.vm_name, "running"),
        )
        .await
        .map_err(|_| WorkerError::new("hot guest eviction exceeded its timeout"))?
    }
}

/// The reset the gate is asked for. One argument, so the daemon's allow-list
/// stays as narrow as the readiness helper's.
const fn simulator_reset(reset: HotReset) -> &'static str {
    match reset.simulator_reset {
        SimulatorReset::None => "none",
        SimulatorReset::Apps => "apps",
        SimulatorReset::Erase => "erase",
    }
}

#[cfg(test)]
mod tests {
    use super::{HotPool, simulator_reset};
    use flanforge_core::{SimulatorReset, VmName};
    use flanforge_manager::{CleanupBudget, HotReset};
    use std::time::Duration;

    fn vm_name() -> VmName {
        VmName::new("ci-hot-project-0123456789ab")
            .unwrap_or_else(|error| unreachable!("fixture: {error}"))
    }

    /// A long-lived child stands in for `tart run`: holding the handle is what
    /// keeps the VM alive, so the handle is the whole of what the pool owns.
    fn guest_process() -> tokio::process::Child {
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30")
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|error| unreachable!("fixture: {error}"))
    }

    /// The pool's hold is what makes cleanup decidable: a guest it declined is
    /// dropped rather than left running with nothing supervising it, and one it
    /// claimed is not cleanup's to drop.
    #[tokio::test]
    async fn a_guest_the_pool_declined_is_dropped_and_a_claimed_one_is_kept() {
        let pool = HotPool::default();
        let name = vm_name();

        pool.park(&name, guest_process()).await;
        assert!(!pool.is_claimed(&name).await);
        assert!(pool.release_unclaimed(&name).await);
        assert!(!pool.release(&name).await, "the declined guest was kept");

        pool.park(&name, guest_process()).await;
        assert!(pool.claim(&name).await);
        assert!(
            !pool.release_unclaimed(&name).await,
            "cleanup dropped a guest the pool had claimed"
        );
        assert!(pool.is_holding(&name).await);
        assert!(pool.release(&name).await);
        assert!(!pool.is_holding(&name).await);
    }

    /// Nothing parked it means the machine is not this daemon's to keep, which
    /// is what a restart leaves behind.
    #[tokio::test]
    async fn an_unparked_machine_cannot_be_claimed() {
        let pool = HotPool::default();
        assert!(!pool.claim(&vm_name()).await);
        assert!(!pool.is_holding(&vm_name()).await);
    }

    /// A child that already exited is a machine that is already gone, whatever
    /// the record says, so the liveness probe must not answer from the claim.
    #[tokio::test]
    async fn a_claimed_guest_whose_process_exited_is_not_held() {
        let pool = HotPool::default();
        let name = vm_name();
        let mut exited = guest_process();
        exited
            .kill()
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));

        pool.park(&name, exited).await;
        assert!(pool.claim(&name).await);
        assert!(
            !pool.is_holding(&name).await,
            "a dead process still read as a live machine"
        );
    }

    /// One argument, so the daemon's helper allow-list stays as narrow as the
    /// readiness helper's.
    #[test]
    fn every_reset_setting_maps_to_one_allowed_argument() {
        for (setting, argument) in [
            (SimulatorReset::None, "none"),
            (SimulatorReset::Apps, "apps"),
            (SimulatorReset::Erase, "erase"),
        ] {
            assert_eq!(
                simulator_reset(HotReset {
                    simulator_reset: setting,
                    budget: CleanupBudget::allow(Duration::from_mins(1)),
                }),
                argument
            );
        }
    }
}
