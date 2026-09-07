use std::time::Duration;

use flanforge_core::{GuestChannelKind, HotGuest, ProfileName, SimulatorReset, VmName};
use flanforge_manager::{CleanupBudget, HotReset, HotRetainRequest, MachineState, WorkerError};
use flanforge_runtime::{
    GuestCapture, GuestCommand, GuestExit, GuestOutput, GuestProgram, RECYCLE_CONTRACT_VERSION,
    RECYCLE_HELPER, RECYCLE_MAX_REPORT_BYTES, RecycleVerdict,
};
use tokio_util::sync::CancellationToken;

use crate::{image::GuestContract, manifest::ensure_matches};

use super::super::{
    LibvirtWorker,
    model::PreparedGuest,
    prepare::load_published_async,
    reap::{find_allocation, load_owned},
};

/// A liveness probe is the one check a claim still pays for, so it is bounded
/// in seconds and reads the same inventory `machines()` does.
const HOT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

impl LibvirtWorker {
    /// Takes a finished allocation's guest into the pool, then proves it came
    /// back clean. There is no process to move: claiming the name is the whole
    /// handover.
    pub(in crate::worker) async fn retain_hot(
        &self,
        request: HotRetainRequest<'_>,
    ) -> Result<(), WorkerError> {
        let name = &request.guest.vm_name;
        if &request.allocation.vm_name != name {
            return Err(WorkerError::new(
                "the hot record does not name this allocation's guest",
            ));
        }
        self.hot.ensure_claimed(name).await;
        if let Err(error) = self.reset_hot(request.guest, request.reset).await {
            // Withdrawing the claim hands the machine back to cleanup, which
            // destroys it the way a non-hot allocation's guest is destroyed.
            self.hot.release(name).await;
            return Err(error);
        }
        Ok(())
    }

    /// Runs the recycle gate over a pool machine and reads its one verdict.
    ///
    /// The daemon runs one named command with a bounded timeout and reads one
    /// JSON line. It knows nothing about what the reset does; the image does.
    pub(in crate::worker) async fn reset_hot(
        &self,
        guest: &HotGuest,
        reset: HotReset,
    ) -> Result<(), WorkerError> {
        let deadline = tokio::time::Instant::now() + reset.budget.timeout();
        let template = self.hot_template(&guest.profile)?;
        // The gate is a bounded teardown-shaped step rather than a job phase,
        // so its budget is the only clock over it and nothing cancels it.
        let prepared = self
            .bind_hot(
                &guest.vm_name,
                &template,
                &CancellationToken::new(),
                deadline,
            )
            .await?;
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
        let output = tokio::time::timeout(
            gate_remaining(deadline)?,
            prepared.job.channel().run(&prepared.session, &command),
        )
        .await
        .map_err(|_| WorkerError::new("the recycle gate exceeded its timeout"))?
        .map_err(|error| WorkerError::new(format!("the recycle gate could not run: {error}")))?;
        let verdict = ensure_clean(&output)?;
        // A machine that came back clean is the pool's, whichever path asked
        // for the gate. It is also how a domain that outlived the daemon
        // rejoins the pool: nothing else repopulates the claim set.
        self.hot.ensure_claimed(&guest.vm_name).await;
        tracing::info!(
            vm_name = %guest.vm_name,
            free_mb = verdict.free_mb(),
            skew_seconds = verdict.skew_seconds(),
            "the recycle gate passed"
        );
        Ok(())
    }

    /// A bounded liveness probe: the pool still claims the domain, and the
    /// host still reports it running.
    pub(in crate::worker) async fn is_hot_alive(&self, guest: &HotGuest) -> bool {
        if !self.hot.is_claimed(&guest.vm_name).await {
            return false;
        }
        let Ok(machines) = self.actor.inventory(HOT_PROBE_TIMEOUT).await else {
            return false;
        };
        machines.iter().any(|machine| {
            machine.name == guest.vm_name.as_str() && machine.state == MachineState::Running
        })
    }

    /// Destroys a pool machine and everything its ownership manifest claims.
    pub(in crate::worker) async fn evict_hot(
        &self,
        guest: &HotGuest,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        // Released first so a retry is not blocked by our own claim. The
        // reaper still cannot collect the domain — the manager keeps the
        // record on failure and every live record protects its name — so the
        // recovery is the manager's own eviction retry on the next sweep.
        self.hot.release(&guest.vm_name).await;
        let allocation_id = find_allocation(&self.state_dir, &guest.vm_name).await?;
        self.ensure_reaped(allocation_id, &guest.vm_name, budget, None)
            .await
    }

    /// Binds a channel to a machine the pool already holds.
    ///
    /// The anchor is the originating allocation's `known_hosts` and alias,
    /// which pooled cleanup deliberately leaves in place: a hot guest keeps the
    /// host key it booted with, so the pin is per hot guest rather than per
    /// allocation. The manifest is re-read here because it, not anything
    /// carried through a job, is the authority over the domain.
    pub(in crate::worker) async fn bind_hot(
        &self,
        name: &VmName,
        template: &VmName,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<PreparedGuest, WorkerError> {
        let allocation_id = find_allocation(&self.state_dir, name).await?;
        let manifest = load_owned(&self.state_dir, allocation_id).await?;
        ensure_matches(
            &manifest,
            allocation_id,
            name,
            &self.instance,
            &self.state_dir,
        )?;
        let contract = GuestContract::read(
            &load_published_async(self.image_manifest_dir.clone(), template.clone()).await?,
        );
        contract.ensure_channel_supported(self.channel, &self.guest_user)?;
        let address = match self.channel {
            GuestChannelKind::Ssh => Some(
                self.wait_for_address(&manifest, cancellation, deadline)
                    .await?,
            ),
            GuestChannelKind::Agent => None,
        };
        self.bind_guest(&manifest, address, &contract)
    }

    /// The template a pool machine's profile boots from, which is where the
    /// guest contract is published. A hot record names a profile rather than an
    /// image, and a profile no live configuration declares fails the gate so
    /// the machine is evicted.
    pub(in crate::worker) fn hot_template(
        &self,
        profile: &ProfileName,
    ) -> Result<VmName, WorkerError> {
        self.current_config()
            .profiles
            .get(profile)
            .map(|profile| profile.template.clone())
            .ok_or_else(|| WorkerError::new("no configured profile claims this hot guest"))
    }
}

/// The gate's one pass condition: it exited zero and reported itself clean.
/// Every other shape names where the reset stopped.
pub(super) fn ensure_clean(output: &GuestOutput) -> Result<RecycleVerdict, WorkerError> {
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
        Ok(verdict)
    } else {
        Err(WorkerError::new(verdict.cause()))
    }
}

/// What the gate's own budget still allows once binding the channel has spent
/// from it.
fn gate_remaining(deadline: tokio::time::Instant) -> Result<Duration, WorkerError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| WorkerError::new("the recycle gate exceeded its timeout"))
}

/// The reset the gate is asked for. One argument, so the guest's allow-list
/// stays as narrow as the readiness helper's.
const fn simulator_reset(reset: HotReset) -> &'static str {
    match reset.simulator_reset {
        SimulatorReset::None => "none",
        SimulatorReset::Apps => "apps",
        SimulatorReset::Erase => "erase",
    }
}
