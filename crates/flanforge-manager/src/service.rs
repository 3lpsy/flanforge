use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationOrigin, AllocationRequest, Config,
    ForgejoClaims, HotDrainReason, HotLane, HotRefusal, Profile, ProfileName, RequestOptions,
    RunnerLabel, resolve_hot_age, resolve_mode, resolve_size,
};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use validator::Validate;

use flanforge_store::{
    AllocationStore, Event, EventKind, EventSink, HotGuestStore, NullEventSink, WarmImageStore,
};

use super::{
    AllocationLeak, AllocationWorker, BusyReason, ConfigHandle, ManagerError, SourceSelection,
    SweepReport,
    admission::{self, MachineProbe},
    hot::{self, HotClaim, HotContext},
    resolve::{profile, vm_name},
    state::prune_terminal_history,
    tuning::ManagerTuning,
};

#[derive(Clone)]
pub struct AllocationManager {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) config: Arc<ConfigHandle>,
    pub(super) store: Arc<dyn AllocationStore>,
    pub(super) images: Arc<dyn WarmImageStore>,
    /// Not optional: a manager that cannot read hot records still runs, and a
    /// reaper that cannot read them deletes hot machines.
    pub(super) hot: Arc<dyn HotGuestStore>,
    /// Where durable "this happened" facts go; appends never fail the caller.
    pub(super) events: Arc<dyn EventSink>,
    pub(super) worker: Arc<dyn AllocationWorker>,
    pub(super) entries: Mutex<HashMap<AllocationId, Entry>>,
    /// Warm images proven unusable this run; deliberately not durable.
    pub(super) quarantined: Mutex<BTreeSet<ProfileName>>,
    pub(super) last_sweep: Mutex<Option<SweepReport>>,
    pub(super) probe: Mutex<MachineProbe>,
    pub(super) is_closing: AtomicBool,
    /// The knobs a test may move; production always leaves them at their
    /// defaults.
    pub(super) tuning: ManagerTuning,
    /// Set once when a shutdown starts. Every teardown path reads it, so a
    /// teardown spawned from the worker guard's `Drop` is bounded by the same
    /// grace as one the worker awaited.
    pub(super) teardown_deadline: OnceLock<tokio::time::Instant>,
    /// Allocations a shutdown could not finish tearing down.
    pub(super) leaked: Mutex<Vec<AllocationLeak>>,
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
    /// A manager that records no events; tests and tools.
    #[must_use]
    pub fn new(
        config: Arc<ConfigHandle>,
        store: Arc<dyn AllocationStore>,
        images: Arc<dyn WarmImageStore>,
        hot: Arc<dyn HotGuestStore>,
        worker: Arc<dyn AllocationWorker>,
    ) -> Self {
        Self::build(
            config,
            store,
            images,
            hot,
            Arc::new(NullEventSink),
            worker,
            ManagerTuning::default(),
        )
    }

    /// The daemon's constructor: state transitions, hot pool movements,
    /// sweeps, and leaks land in the given sink.
    #[must_use]
    pub fn new_with_events(
        config: Arc<ConfigHandle>,
        store: Arc<dyn AllocationStore>,
        images: Arc<dyn WarmImageStore>,
        hot: Arc<dyn HotGuestStore>,
        events: Arc<dyn EventSink>,
        worker: Arc<dyn AllocationWorker>,
    ) -> Self {
        Self::build(
            config,
            store,
            images,
            hot,
            events,
            worker,
            ManagerTuning::default(),
        )
    }

    #[cfg(test)]
    pub(super) fn with_tuning(
        config: Arc<ConfigHandle>,
        store: Arc<dyn AllocationStore>,
        images: Arc<dyn WarmImageStore>,
        hot: Arc<dyn HotGuestStore>,
        worker: Arc<dyn AllocationWorker>,
        tuning: ManagerTuning,
    ) -> Self {
        Self::build(
            config,
            store,
            images,
            hot,
            Arc::new(NullEventSink),
            worker,
            tuning,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: Arc<ConfigHandle>,
        store: Arc<dyn AllocationStore>,
        images: Arc<dyn WarmImageStore>,
        hot: Arc<dyn HotGuestStore>,
        events: Arc<dyn EventSink>,
        worker: Arc<dyn AllocationWorker>,
        tuning: ManagerTuning,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                store,
                images,
                hot,
                events,
                worker,
                entries: Mutex::new(HashMap::new()),
                quarantined: Mutex::new(BTreeSet::new()),
                last_sweep: Mutex::new(None),
                probe: Mutex::new(MachineProbe::default()),
                is_closing: AtomicBool::new(false),
                tuning,
                teardown_deadline: OnceLock::new(),
                leaked: Mutex::new(Vec::new()),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn config_handle(&self) -> &Arc<ConfigHandle> {
        &self.inner.config
    }

    /// Records one durable event; implementations never fail the caller.
    pub(super) async fn emit(&self, event: Event) {
        self.inner.events.record(event).await;
    }

    /// A live view of one resident allocation, or `None` once it left the
    /// window; the substrate the web UI's per-allocation stream rides on.
    pub async fn watch_allocation(&self, id: AllocationId) -> Option<watch::Receiver<Allocation>> {
        self.inner
            .entries
            .lock()
            .await
            .get(&id)
            .map(|entry| entry.sender.subscribe())
    }

    /// Gives a claim back when the allocation it was made for never became
    /// durable. The machine returns through the drain path, which destroys it
    /// rather than guessing that it is still clean.
    async fn abandon_hot_claim(&self, claim: Option<&HotClaim>) {
        if let Some(HotClaim::Reused { guest, .. }) = claim {
            self.ensure_hot_drained(guest, HotDrainReason::ResetFailed)
                .await;
        }
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
        let config = self.inner.config.current();
        let resolved = self.authorize(&config, &request, options, claims).await?;
        let hot_context = self
            .prepare_hot(&config, &request, &resolved, options)
            .await;
        let Resolved {
            profile,
            size,
            mode,
            source,
            lane,
            age_seconds,
        } = resolved;

        // Probed before the lock; classified under it, against this snapshot.
        // Read after both evictions above, so the snapshot cannot charge a
        // slot this request just freed.
        let machines = self.host_machines().await;
        let hot = self.inner.hot.load_all().await;

        let mut entries = self.inner.entries.lock().await;
        if self.inner.is_closing.load(Ordering::Acquire) {
            return Err(ManagerError::ShuttingDown);
        }
        if let Some(existing) = idempotent(&entries, &request) {
            return Ok(existing);
        }
        Self::ensure_sole_producer(&entries, &request.profile, mode)?;
        let machines = machines.map_err(|_| admission::busy(BusyReason::ProbeFailed, None))?;
        let hot = admission::hot_snapshot(hot, &config)?;
        Self::ensure_capacity(&entries, &config, size, &machines, &hot)?;

        let runner_label =
            RunnerLabel::new(format!("{}-{}", profile.runner_label, uuid::Uuid::new_v4()))
                .map_err(|error| ManagerError::InvalidRunnerLabel(error.to_string()))?;
        let vm_name = vm_name(&config, &request)?;
        let mut allocation = Allocation::new(request, vm_name, runner_label, mode, size);
        if !self.inner.worker.is_clone_source_evidence_deferred() {
            allocation.set_clone_source(source.source, source.warm_generation);
        }
        // The claim itself is a pure state decision against the snapshot and
        // the names live entries already hold; only its commit touches a store,
        // beside the allocation's own commit so one decision leaves one pair of
        // records or neither.
        let claim = hot_context
            .as_ref()
            .and_then(|context| hot::plan_hot_claim(&entries, &hot, context, allocation.id));
        let decision = HotDecision {
            claim: claim.as_ref(),
            context: hot_context.as_ref(),
            lane,
            age_seconds,
            mode,
            max_hot_vms: config.runtime.max_hot_vms,
        };
        decision.apply(&mut allocation, options, &profile);
        if let Some(HotClaim::Reused { guest, .. }) = &claim {
            self.inner.hot.save(guest).await?;
            self.emit(
                Event::new(EventKind::HotClaimed)
                    .with_allocation(allocation.id)
                    .with_vm_name(guest.vm_name.clone())
                    .with_profile(guest.profile.clone())
                    .with_payload(&serde_json::json!({"jobs_served": guest.jobs_served})),
            )
            .await;
        }
        if let Err(error) = self.inner.store.save(&allocation).await {
            self.abandon_hot_claim(claim.as_ref()).await;
            return Err(error.into());
        }
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
        prune_terminal_history(&mut entries, self.inner.tuning.history_limit);
        drop(entries);

        tracing::info!(
            allocation_id = %allocation.id,
            repository = %allocation.request.repository,
            profile = %allocation.request.profile,
            run_id = allocation.request.run_id,
            run_attempt = allocation.request.run_attempt,
            vm_name = %allocation.vm_name,
            hot = ?allocation.origin.hot_vm_name(),
            "allocation created"
        );
        self.spawn_worker(allocation.clone(), profile, cancellation);
        Ok(CreateAllocation {
            allocation,
            receiver,
            is_new: true,
        })
    }

    /// Authorizes the request against its profile and resolves everything the
    /// admission fold and the record both need. No host, no store, no lock:
    /// this is the phase that can refuse on policy alone.
    async fn authorize(
        &self,
        config: &Config,
        request: &AllocationRequest,
        options: RequestOptions,
        claims: &ForgejoClaims,
    ) -> Result<Resolved, ManagerError> {
        request
            .validate()
            .map_err(|_| ManagerError::InvalidRequest)?;
        let profile = profile(config, request)?;
        claims.ensure_authorized(&profile, request)?;
        // Both ceilings are rejections rather than clamps: silently granting
        // less than a workflow asked for is worse than refusing.
        let size =
            resolve_size(options, &profile).map_err(|_| ManagerError::RequestExceedsProfile)?;
        let age_seconds = resolve_hot_age(options.hot, &profile)
            .map_err(|_| ManagerError::RequestExceedsProfile)?;
        let mode = resolve_mode(
            options,
            &profile,
            claims.workflow_file().unwrap_or_default(),
        );
        let source = self.resolve_source(&request.profile, &profile, mode).await;
        // The guest's disk is floored at its base, so that floor is what the
        // budget must reserve and what the record has to carry.
        let size = self.effective_size(size, &request.profile, &source).await;
        tracing::debug!(
            repository = %request.repository,
            profile = %request.profile,
            run_id = request.run_id,
            run_attempt = request.run_attempt,
            ?mode,
            source = %source.name,
            "allocation request authorized"
        );
        Ok(Resolved {
            lane: HotLane::from_ref_protected(claims.is_ref_protected()),
            profile,
            size,
            mode,
            source,
            age_seconds,
        })
    }

    /// Runs everything a `hot` request asks for *before* admission folds, and
    /// answers whether this allocation may claim or retain a machine.
    ///
    /// The eviction and the capacity yield both have to happen here rather
    /// than after, and the ordering is load-bearing rather than tidy. On a host
    /// with `max_running_vms = 1` and one retained machine holding the only
    /// slot, a deferred teardown deadlocks: the incoming allocation cannot be
    /// admitted until the slot is free, the slot is not freed until the
    /// allocation completes, and nothing but the reaper's one-hour age floor
    /// can break the cycle. Freeing the slot first makes it unformable.
    async fn prepare_hot(
        &self,
        config: &Config,
        request: &AllocationRequest,
        resolved: &Resolved,
        options: RequestOptions,
    ) -> Option<HotContext> {
        // The workflow is the authority on when a hot session ends, and it says
        // so explicitly: only `hot: "evict"` tears the pool down. A run that
        // simply does not mention hot leaves every retained machine alone,
        // because it may be one another workflow is relying on.
        if options.hot.is_evicting() {
            self.ensure_hot_evicted_for(&request.profile, HotDrainReason::EvictRequested)
                .await;
        }
        // A retained machine is a cache, and a cache yields under pressure.
        // Without this, one machine on a one-slot host refuses every allocation
        // until it ages out.
        self.ensure_hot_yielded(config, resolved.size).await;
        match resolved.mode {
            AllocationMode::Cold | AllocationMode::Warm if options.hot.is_retaining() => {
                self.hot_context(
                    config,
                    &request.profile,
                    &resolved.profile,
                    resolved.lane,
                    resolved.age_seconds,
                )
                .await
            }
            _ => None,
        }
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
        self.cancel_by_id(id).await
    }
}

/// Everything `authorize` settled: the policy decisions the admission fold and
/// the record both read, none of which needs a host or a lock.
struct Resolved {
    profile: Profile,
    size: flanforge_core::GuestSize,
    mode: AllocationMode,
    source: SourceSelection,
    lane: HotLane,
    age_seconds: Option<u64>,
}

/// The live allocation for this same run attempt, if one is still going. Two
/// requests for one attempt get one allocation, whatever the second asked for.
fn idempotent(
    entries: &HashMap<AllocationId, Entry>,
    request: &AllocationRequest,
) -> Option<CreateAllocation> {
    let entry = entries.values().find(|entry| {
        !entry.allocation.state.is_terminal() && entry.allocation.request.is_same_attempt(request)
    })?;
    tracing::info!(
        allocation_id = %entry.allocation.id,
        state = ?entry.allocation.state,
        "returning idempotent allocation"
    );
    Some(CreateAllocation {
        allocation: entry.allocation.clone(),
        receiver: entry.sender.subscribe(),
        is_new: false,
    })
}

/// What admission decided about one `hot` request, in the shape the record
/// takes it. Kept together so a lane without an outcome and an outcome without
/// a lane are both unspellable.
struct HotDecision<'a> {
    claim: Option<&'a HotClaim>,
    context: Option<&'a HotContext>,
    lane: HotLane,
    age_seconds: Option<u64>,
    mode: AllocationMode,
    max_hot_vms: u8,
}

impl HotDecision<'_> {
    /// A refused preference never fails the request: it is recorded and the
    /// allocation runs as an ordinary one, exactly as a warm request that
    /// finds no usable image boots cold.
    fn apply(&self, allocation: &mut Allocation, options: RequestOptions, profile: &Profile) {
        match self.claim {
            Some(HotClaim::Reused { origin, source, .. }) => allocation.set_hot(
                Some(self.lane),
                origin.clone(),
                Some(source.clone()),
                self.age_seconds,
            ),
            Some(HotClaim::Retain) => allocation.set_hot(
                Some(self.lane),
                AllocationOrigin::Cloned,
                None,
                self.age_seconds,
            ),
            // A context that exists but yielded no claim means the pool had
            // no room; no context at all means the profile or the backend
            // refused, and only then is there a reason to derive.
            None if options.hot.is_retaining() => {
                let refusal = match self.context {
                    Some(_) => HotRefusal::PoolFull,
                    None => refusal(self.mode, profile, self.lane, self.max_hot_vms),
                };
                allocation.set_hot_refused(refusal);
            }
            None => {}
        }
    }
}

/// Why a `hot` request produced no pool machine, when the profile or the
/// backend is what refused rather than the pool being full.
fn refusal(mode: AllocationMode, profile: &Profile, lane: HotLane, max_hot_vms: u8) -> HotRefusal {
    if mode == AllocationMode::Regenerate {
        return HotRefusal::Regeneration;
    }
    match profile.hot.filter(|hot| hot.enabled) {
        None => HotRefusal::NotEnabled,
        Some(hot) if !hot.lanes.is_lane_admitted(lane) => HotRefusal::LaneNotAdmitted,
        // The global cap leaves the pool no room, which is this refusal's own
        // case. Only a backend that declares nothing is `Unsupported`.
        Some(_) if max_hot_vms == 0 => HotRefusal::PoolFull,
        Some(_) => HotRefusal::Unsupported,
    }
}
