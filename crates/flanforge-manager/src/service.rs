use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use flanforge_core::{
    Allocation, AllocationId, AllocationRequest, ForgejoClaims, ProfileName, RequestOptions,
    RunnerLabel, resolve_mode, resolve_size,
};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use validator::Validate;

use flanforge_store::{AllocationStore, WarmImageStore};

use super::{
    AllocationWorker, BusyReason, ConfigHandle, ManagerError, SweepReport,
    admission::MachineProbe,
    resolve::{profile, vm_name},
    state::prune_terminal_history,
};

pub(super) const IN_MEMORY_ALLOCATION_LIMIT: usize = 1_000;

#[derive(Clone)]
pub struct AllocationManager {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) config: Arc<ConfigHandle>,
    pub(super) store: Arc<dyn AllocationStore>,
    pub(super) images: Arc<dyn WarmImageStore>,
    pub(super) worker: Arc<dyn AllocationWorker>,
    pub(super) entries: Mutex<HashMap<AllocationId, Entry>>,
    /// Warm images proven unusable this run; deliberately not durable.
    pub(super) quarantined: Mutex<BTreeSet<ProfileName>>,
    pub(super) last_sweep: Mutex<Option<SweepReport>>,
    pub(super) probe: Mutex<MachineProbe>,
    pub(super) is_closing: AtomicBool,
}

pub(super) struct Entry {
    pub(super) allocation: Allocation,
    pub(super) sender: watch::Sender<Allocation>,
    pub(super) cancellation: CancellationToken,
    /// False for a recovered entry, and cleared when the worker task exits, so
    /// it answers "is a task listening?" rather than "who created this?".
    pub(super) is_supervised: bool,
    /// Set while one caller is driving this entry to a terminal state, so a
    /// second cancel awaits it instead of running cleanup a second time.
    pub(super) is_terminalizing: bool,
}

impl std::fmt::Debug for AllocationManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AllocationManager")
            .field("profiles", &self.inner.config.current().profiles.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct CreateAllocation {
    pub allocation: Allocation,
    pub receiver: watch::Receiver<Allocation>,
    pub is_new: bool,
}

impl AllocationManager {
    #[must_use]
    pub fn new(
        config: Arc<ConfigHandle>,
        store: Arc<dyn AllocationStore>,
        images: Arc<dyn WarmImageStore>,
        worker: Arc<dyn AllocationWorker>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                store,
                images,
                worker,
                entries: Mutex::new(HashMap::new()),
                quarantined: Mutex::new(BTreeSet::new()),
                last_sweep: Mutex::new(None),
                probe: Mutex::new(MachineProbe::default()),
                is_closing: AtomicBool::new(false),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn config_handle(&self) -> &Arc<ConfigHandle> {
        &self.inner.config
    }

    /// Creates an authorized allocation or returns its idempotent predecessor.
    ///
    /// # Errors
    ///
    /// Returns an error for policy denial, conflicting active work, invalid VM
    /// names, or persistence failures.
    pub async fn create(
        &self,
        request: AllocationRequest,
        options: RequestOptions,
        claims: &ForgejoClaims,
    ) -> Result<CreateAllocation, ManagerError> {
        request
            .validate()
            .map_err(|_| ManagerError::InvalidRequest)?;
        let config = self.inner.config.current();
        let profile = profile(&config, &request)?;
        claims.ensure_authorized(&profile, &request)?;
        let size =
            resolve_size(options, &profile).map_err(|_| ManagerError::RequestExceedsProfile)?;
        let mode = resolve_mode(
            options,
            &profile,
            claims.workflow_file().unwrap_or_default(),
        );
        let source = self.resolve_source(&request.profile, &profile, mode).await;
        tracing::debug!(
            repository = %request.repository,
            profile = %request.profile,
            run_id = request.run_id,
            run_attempt = request.run_attempt,
            ?mode,
            source = %source.name,
            "allocation request authorized"
        );

        // Probed before the lock; classified under it, against this snapshot.
        let machines = self.host_machines().await;

        let mut entries = self.inner.entries.lock().await;
        if self.inner.is_closing.load(Ordering::Acquire) {
            return Err(ManagerError::ShuttingDown);
        }
        if let Some(entry) = entries
            .values()
            .find(|entry| entry.allocation.request.is_same_attempt(&request))
        {
            tracing::info!(
                allocation_id = %entry.allocation.id,
                state = ?entry.allocation.state,
                "returning idempotent allocation"
            );
            return Ok(CreateAllocation {
                allocation: entry.allocation.clone(),
                receiver: entry.sender.subscribe(),
                is_new: false,
            });
        }
        Self::ensure_sole_producer(&entries, &request.profile, mode)?;
        let machines = machines.map_err(|_| ManagerError::Busy {
            reason: BusyReason::ProbeFailed,
            holder: None,
        })?;
        Self::ensure_capacity(&entries, &config, size, &machines)?;

        let vm_name = vm_name(&config, &request)?;
        let runner_label =
            RunnerLabel::new(format!("{}-{}", profile.runner_label, uuid::Uuid::new_v4()))
                .map_err(|error| ManagerError::InvalidRunnerLabel(error.to_string()))?;
        let mut allocation = Allocation::new(request, vm_name, runner_label, mode, size);
        allocation.set_source(source);
        self.inner.store.save(&allocation).await?;
        let (sender, receiver) = watch::channel(allocation.clone());
        let cancellation = CancellationToken::new();
        entries.insert(
            allocation.id,
            Entry {
                allocation: allocation.clone(),
                sender,
                cancellation: cancellation.clone(),
                is_supervised: true,
                is_terminalizing: false,
            },
        );
        prune_terminal_history(&mut entries);
        drop(entries);

        tracing::info!(
            allocation_id = %allocation.id,
            repository = %allocation.request.repository,
            profile = %allocation.request.profile,
            run_id = allocation.request.run_id,
            run_attempt = allocation.request.run_attempt,
            vm_name = %allocation.vm_name,
            "allocation created"
        );
        self.spawn_worker(allocation.clone(), profile, cancellation);
        Ok(CreateAllocation {
            allocation,
            receiver,
            is_new: true,
        })
    }

    /// Returns an allocation after rechecking the caller's signed claims.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is absent or unauthorized.
    pub async fn get_authorized(
        &self,
        id: AllocationId,
        claims: &ForgejoClaims,
    ) -> Result<Allocation, ManagerError> {
        let allocation = self.get(id).await?;
        let config = self.inner.config.current();
        let profile = profile(&config, &allocation.request)?;
        claims.ensure_authorized(&profile, &allocation.request)?;
        Ok(allocation)
    }

    /// Requests cancellation after rechecking the caller's signed claims.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is absent or unauthorized.
    pub async fn cancel_authorized(
        &self,
        id: AllocationId,
        claims: &ForgejoClaims,
    ) -> Result<Allocation, ManagerError> {
        let allocation = self.get_authorized(id, claims).await?;
        if allocation.state.is_terminal() {
            return Ok(allocation);
        }
        let entries = self.inner.entries.lock().await;
        let entry = entries.get(&id).ok_or(ManagerError::NotFound(id))?;
        entry.cancellation.cancel();
        tracing::info!(allocation_id = %id, state = ?entry.allocation.state, "allocation cancellation signalled");
        Ok(entry.allocation.clone())
    }
}
