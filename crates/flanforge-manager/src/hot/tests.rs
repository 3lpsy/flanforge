use std::collections::{BTreeSet, HashMap};

use flanforge_core::{
    Allocation, AllocationId, AllocationMode, AllocationOrigin, AllocationState, CloneKind,
    CloneSource, HotConfig, HotDrainReason, HotGuest, HotLane, HotLanePolicy, HotState, Profile,
    ProfileName, RunnerLabel, VmName, is_hot_draining_change,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use flanforge_test_support as test_support;

use super::{
    super::{AllocationManager, service::Entry},
    model::HotContext,
    select::{HotPolicy, claimable, drain_reason, is_pool_expandable, surplus_idle},
};
use flanforge_orm::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};

fn hot_config() -> HotConfig {
    HotConfig {
        enabled: true,
        lanes: HotLanePolicy::Protected,
        max_idle: 1,
        max_lifetime_seconds: 14_400,
        max_jobs: 20,
        idle_ttl_seconds: 900,
        reset_timeout_seconds: 120,
        simulator_reset: flanforge_core::SimulatorReset::Apps,
    }
}

fn guest(name: &str, jobs_served: u32) -> HotGuest {
    let mut guest = HotGuest::new(
        VmName::new(name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        test_support::profile_name(),
        HotLane::Protected,
        test_support::size(),
        CloneSource {
            name: VmName::new("flanforge-base")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            kind: CloneKind::Template,
            base_fingerprint: None,
            fallback_reason: None,
        },
        None,
        None,
    );
    guest.state = HotState::Idle;
    guest.jobs_served = jobs_served;
    guest
}

/// `now` is the record's own clock, so a bound is measured from where the
/// fixture actually put the machine rather than from a wall clock the test
/// cannot control.
fn policy_at(hot: &HotConfig, now: u64) -> HotPolicy<'_> {
    HotPolicy {
        hot,
        fingerprint: None,
        warm_generation: None,
        now,
    }
}

/// An allocation bound to a pool machine, exactly as `create` leaves it.
fn hot_allocation(vm_name: &str) -> Allocation {
    let mut allocation = Allocation::new(
        test_support::request(),
        VmName::new("ci-project-1-1").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        RunnerLabel::new("label").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        AllocationMode::Cold,
        test_support::size(),
    );
    allocation.state = AllocationState::Registering;
    allocation.origin = AllocationOrigin::HotReuse {
        vm_name: VmName::new(vm_name).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jobs_served: 1,
        booted_at_unix: 0,
    };
    allocation
}

fn entries(allocations: Vec<Allocation>) -> HashMap<AllocationId, Entry> {
    allocations
        .into_iter()
        .map(|allocation| {
            let (sender, _) = watch::channel(allocation.clone());
            (
                allocation.id,
                Entry {
                    allocation,
                    sender,
                    cancellation: CancellationToken::new(),
                    is_supervised: true,
                    is_terminalizing: false,
                },
            )
        })
        .collect()
}

/// The race the whole design rests on: two `create` calls folding under one
/// lock must never leave with the same machine. The first inserts its entry
/// before releasing the lock, and that entry is what the second one sees.
#[test]
fn two_concurrent_claims_cannot_take_one_machine() {
    let hot = [guest("ci-hot-project-0123456789ab", 3)];
    let config = hot_config();
    let name = test_support::profile_name();
    let empty = BTreeSet::new();

    let first = claimable(
        &hot,
        &name,
        HotLane::Protected,
        &empty,
        policy_at(&config, flanforge_core::unix_time()),
    )
    .unwrap_or_else(|| unreachable!("an idle machine is claimable"));
    assert_eq!(first.vm_name.as_str(), "ci-hot-project-0123456789ab");

    // The first claim is now visible through the entries, which is the only
    // place a claim exists before its record is committed.
    let taken = entries(vec![hot_allocation("ci-hot-project-0123456789ab")]);
    let claimed = super::claim::live_hot_names(&taken);
    assert!(claimed.contains("ci-hot-project-0123456789ab"));
    assert!(
        claimable(
            &hot,
            &name,
            HotLane::Protected,
            &claimed,
            policy_at(&config, flanforge_core::unix_time())
        )
        .is_none(),
        "the second claim must not take the machine the first one holds"
    );
}

/// Profile pinning is absolute and the lane is derived from a signed claim, so
/// neither is negotiable at selection time.
#[test]
fn a_claim_never_crosses_a_profile_or_a_lane() {
    let hot = [guest("ci-hot-project-0123456789ab", 0)];
    let config = hot_config();
    let empty = BTreeSet::new();
    let other = ProfileName::new("other").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(
        claimable(
            &hot,
            &other,
            HotLane::Protected,
            &empty,
            policy_at(&config, flanforge_core::unix_time())
        )
        .is_none()
    );
    assert!(
        claimable(
            &hot,
            &test_support::profile_name(),
            HotLane::Unprotected,
            &empty,
            policy_at(&config, flanforge_core::unix_time())
        )
        .is_none()
    );
}

/// Wear concentrates on one machine rather than spreading, so the pool retires
/// machines one at a time instead of all at once.
#[test]
fn the_machine_closest_to_its_job_bound_is_claimed_first() {
    let hot = [
        guest("ci-hot-project-0123456789ab", 1),
        guest("ci-hot-project-ba9876543210", 7),
    ];
    let config = hot_config();
    let claimed = claimable(
        &hot,
        &test_support::profile_name(),
        HotLane::Protected,
        &BTreeSet::new(),
        policy_at(&config, flanforge_core::unix_time()),
    )
    .unwrap_or_else(|| unreachable!("one of the two is claimable"));
    assert_eq!(claimed.vm_name.as_str(), "ci-hot-project-ba9876543210");
}

#[test]
fn every_configured_bound_names_the_reason_it_drained() {
    let config = hot_config();
    let booted = guest("ci-hot-project-0123456789ab", 0).booted_at_unix;

    let over_lifetime = guest("ci-hot-project-0123456789ab", 0);
    assert_eq!(
        drain_reason(&over_lifetime, policy_at(&config, booted + 14_400)),
        Some(HotDrainReason::MaxLifetime)
    );

    let spent = guest("ci-hot-project-0123456789ab", 20);
    assert_eq!(
        drain_reason(&spent, policy_at(&config, booted + 1)),
        Some(HotDrainReason::MaxJobs)
    );

    let mut stale = guest("ci-hot-project-0123456789ab", 0);
    stale.updated_at_unix = booted;
    assert_eq!(
        drain_reason(&stale, policy_at(&config, booted + 900)),
        Some(HotDrainReason::IdleTtl)
    );

    // A machine built from a warm generation the profile no longer promotes is
    // not policy, it is correctness: it is not the machine hot was configured
    // to hand out.
    let mut superseded = guest("ci-hot-project-0123456789ab", 0);
    superseded.warm_generation = Some(3);
    assert_eq!(
        drain_reason(&superseded, policy_at(&config, booted + 1)),
        Some(HotDrainReason::StaleBase)
    );

    assert_eq!(
        drain_reason(
            &guest("ci-hot-project-0123456789ab", 0),
            policy_at(&config, flanforge_core::unix_time())
        ),
        None
    );
}

/// A drained machine is never claimable, whatever else the record says.
#[test]
fn a_machine_past_a_bound_is_not_claimable() {
    let hot = [guest("ci-hot-project-0123456789ab", 20)];
    let config = hot_config();
    assert!(
        claimable(
            &hot,
            &test_support::profile_name(),
            HotLane::Protected,
            &BTreeSet::new(),
            policy_at(&config, hot[0].booted_at_unix + 1)
        )
        .is_none()
    );
}

/// The stated precedence: `runtime.max_hot_vms` is a fact about the host and
/// `max_idle` is a per-lane ceiling applied at release, so the global cap is
/// the only thing admission asks about.
#[test]
fn the_global_cap_is_what_admission_checks() {
    let name = test_support::profile_name();
    let held = [guest("ci-project-1-1", 0)];

    assert!(
        !is_pool_expandable(&held, 1, &[]),
        "one machine already fills a global cap of one"
    );
    assert!(is_pool_expandable(&held, 2, &[]));
    // A retention another `create` decided under this same lock is not in the
    // record snapshot, so it has to be counted from the entries instead.
    assert!(
        !is_pool_expandable(&held, 2, &[&name]),
        "an in-flight retention counts against the cap"
    );
}

/// A release that overshoots retires the stalest machine, not the one just
/// recycled: the freshest machine is the one whose caches are warmest.
#[test]
fn surplus_idle_retires_the_stalest_machine_first() {
    let config = hot_config();
    let mut stale = guest("ci-hot-project-0123456789ab", 0);
    stale.updated_at_unix = 100;
    let mut fresh = guest("ci-hot-project-ba9876543210", 0);
    fresh.updated_at_unix = 900;
    let pool = [stale, fresh];
    let surplus = surplus_idle(
        &pool,
        &test_support::profile_name(),
        HotLane::Protected,
        &config,
    );
    assert_eq!(surplus.len(), 1);
    assert_eq!(surplus[0].vm_name.as_str(), "ci-hot-project-0123456789ab");
}

/// The whole feature, end to end over the record lifecycle: a hot allocation's
/// guest is retained, the next request for the same profile and lane claims it,
/// and the job count follows the machine rather than the allocation.
#[tokio::test]
async fn a_retained_guest_is_claimed_by_the_next_request_for_its_lane() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::hot_config(directory.path().to_path_buf());
    let manager = manager_with(directory.path(), Arc::clone(&config), Arc::new(GatedWorker)).await;
    let profile = config
        .profiles
        .get(&test_support::profile_name())
        .unwrap_or_else(|| unreachable!("fixture profile"))
        .clone();

    // A finished hot allocation hands its guest over.
    let mut first = hot_allocation("ci-project-1-1");
    first.origin = AllocationOrigin::Cloned;
    first.hot_lane = Some(HotLane::Protected);
    first.vm_created = true;
    first.source = Some(source());
    manager.ensure_hot_released(&first, &profile).await;

    let pool = manager
        .inner
        .hot
        .load_all()
        .await
        .unwrap_or_else(|error| unreachable!("pool: {error}"));
    assert_eq!(pool.len(), 1, "the finished guest joined the pool");
    assert_eq!(pool[0].vm_name.as_str(), "ci-project-1-1");
    assert_eq!(pool[0].state, HotState::Idle);
    assert_eq!(pool[0].jobs_served, 1, "the job it just ran is its first");

    // The next request for the same profile and lane claims it.
    let context = manager
        .hot_context(
            &config,
            &test_support::profile_name(),
            &profile,
            HotLane::Protected,
            None,
        )
        .await
        .unwrap_or_else(|| unreachable!("hot is enabled and supported"));
    let claim = super::claim::plan_hot_claim(&HashMap::new(), &pool, &context, AllocationId::new())
        .unwrap_or_else(|| unreachable!("an idle machine is claimable"));
    let super::HotClaim::Reused { guest, origin, .. } = claim else {
        unreachable!("a machine in the pool is reused, not retained again")
    };
    assert_eq!(guest.state, HotState::Claimed);
    assert_eq!(
        origin.hot_vm_name().map(flanforge_core::VmName::as_str),
        Some("ci-project-1-1")
    );

    // The other lane cannot have this machine, and with `max_hot_vms = 1`
    // already held it cannot retain one of its own either — which is the
    // refusal an operator reads as `PoolFull`.
    assert!(
        super::claim::plan_hot_claim(
            &HashMap::new(),
            &pool,
            &HotContext {
                lane: HotLane::Unprotected,
                ..context
            },
            AllocationId::new(),
        )
        .is_none()
    );
}

/// `ensure_terminal` re-enters when a pass fails at its final transition, so
/// the release has to be idempotent. A second pass that evicted would destroy
/// a machine the pool legitimately holds — possibly one already re-claimed.
#[tokio::test]
async fn releasing_the_same_allocation_twice_leaves_the_pool_alone() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::hot_config(directory.path().to_path_buf());
    let manager = manager_with(directory.path(), Arc::clone(&config), Arc::new(GatedWorker)).await;
    let profile = config
        .profiles
        .get(&test_support::profile_name())
        .unwrap_or_else(|| unreachable!("fixture profile"))
        .clone();

    let mut retained = hot_allocation("ci-project-1-1");
    retained.origin = AllocationOrigin::Cloned;
    retained.hot_lane = Some(HotLane::Protected);
    retained.vm_created = true;
    retained.source = Some(source());
    manager.ensure_hot_released(&retained, &profile).await;
    manager.ensure_hot_released(&retained, &profile).await;

    let pool = manager
        .inner
        .hot
        .load_all()
        .await
        .unwrap_or_else(|error| unreachable!("pool: {error}"));
    assert_eq!(
        pool.len(),
        1,
        "the second retention was not a second record"
    );
    assert_eq!(pool[0].state, HotState::Idle);
    assert_eq!(pool[0].jobs_served, 1, "the job was not counted twice");

    // Now the reuse arm, against the record the retention left behind.
    let mut reuser = hot_allocation("ci-project-1-1");
    reuser.origin = AllocationOrigin::HotReuse {
        vm_name: VmName::new("ci-project-1-1")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jobs_served: 1,
        booted_at_unix: pool[0].booted_at_unix,
    };
    let mut claimed = pool[0].clone();
    claimed
        .ensure_claimed(reuser.id)
        .unwrap_or_else(|error| unreachable!("claim: {error}"));
    manager
        .inner
        .hot
        .save(&claimed)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    manager.ensure_hot_released(&reuser, &profile).await;
    manager.ensure_hot_released(&reuser, &profile).await;

    let pool = manager
        .inner
        .hot
        .load_all()
        .await
        .unwrap_or_else(|error| unreachable!("pool: {error}"));
    assert_eq!(
        pool.len(),
        1,
        "the second release did not evict the machine"
    );
    assert_eq!(pool[0].state, HotState::Idle);
    assert_eq!(
        pool[0].jobs_served, 2,
        "the reused job counted exactly once"
    );
}

/// A workflow that does not mention hot must leave the pool alone: a machine
/// another workflow is relying on is not this one's to destroy.
#[test]
fn an_untouched_request_neither_claims_nor_retains() {
    let options = flanforge_core::RequestOptions::default();
    assert!(!options.hot.is_retaining());
    assert!(!options.hot.is_evicting());
    assert_eq!(options.hot, flanforge_core::HotRequest::Untouched);
}

/// The ceiling is on what a run may ask for, not the lifetime itself, and a
/// request above it is refused rather than quietly shortened.
#[test]
fn a_requested_lifetime_above_the_profile_ceiling_is_rejected_not_clamped() {
    let profile = test_support::hot_profile();
    let ceiling = profile
        .hot
        .unwrap_or_else(|| unreachable!("fixture hot table"))
        .max_lifetime_seconds;

    assert_eq!(
        flanforge_core::resolve_hot_age(
            flanforge_core::HotRequest::Retain {
                age_seconds: Some(ceiling)
            },
            &profile
        ),
        Ok(Some(ceiling))
    );
    assert!(
        flanforge_core::resolve_hot_age(
            flanforge_core::HotRequest::Retain {
                age_seconds: Some(ceiling + 1)
            },
            &profile
        )
        .is_err()
    );
    // Unset takes the ceiling, which is the only place the default lives.
    assert_eq!(
        flanforge_core::resolve_hot_age(
            flanforge_core::HotRequest::Retain { age_seconds: None },
            &profile
        ),
        Ok(None)
    );
    // A profile that does not enable hot has no ceiling to exceed, so the age
    // is irrelevant: the request is refused as `NotEnabled` and the allocation
    // runs. Failing it would be the one hot refusal that fails a job.
    assert_eq!(
        flanforge_core::resolve_hot_age(
            flanforge_core::HotRequest::Retain {
                age_seconds: Some(ceiling * 100)
            },
            &test_support::profile()
        ),
        Ok(None)
    );
}

/// `Unsupported` says the backend cannot hold a machine open. A backend that
/// can, under a cap of zero, is a full pool — which is what `PoolFull` is for,
/// and what an operator has to read to know which knob to move.
#[tokio::test]
async fn the_global_kill_switch_refuses_a_request_as_a_full_pool() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = Arc::new(capped(directory.path(), 0));
    let manager = manager_with(directory.path(), config, Arc::new(GatedWorker)).await;

    let created = manager
        .create(
            test_support::request(),
            flanforge_core::RequestOptions {
                hot: flanforge_core::HotRequest::Retain { age_seconds: None },
                ..flanforge_core::RequestOptions::default()
            },
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    assert_eq!(
        created.allocation.hot_refusal,
        Some(flanforge_core::HotRefusal::PoolFull)
    );
    assert!(created.allocation.hot_lane.is_none());
}

/// Regeneration must be unable to claim, not merely refused: `create` reads
/// the mode from the signed workflow file and never builds a context for it.
/// This asserts the manager's own gate on top of that.
#[tokio::test]
async fn a_backend_without_the_capability_admits_no_hot_context() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::hot_config(directory.path().to_path_buf());
    let manager = AllocationManager::new(
        ConfigHandle::new(Arc::clone(&config)),
        Arc::new(
            SqliteAllocationStore::open(&directory.path().join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::new(
            SqliteWarmImageStore::open(&directory.path().join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::new(
            SqliteHotGuestStore::open(&directory.path().join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::new(ColdOnlyWorker),
    );
    let profile = config
        .profiles
        .get(&test_support::profile_name())
        .unwrap_or_else(|| unreachable!("fixture profile"));
    assert!(
        manager
            .hot_context(
                &config,
                &test_support::profile_name(),
                profile,
                HotLane::Protected,
                None
            )
            .await
            .is_none(),
        "a backend that cannot hold a machine open must not be handed one"
    );
}

/// Every field a running machine cannot absorb, in one predicate with its own
/// test, so the drain site reads one call rather than a field list.
#[test]
fn a_reload_that_resizes_or_repoints_a_profile_drains_it() {
    let base = test_support::hot_profile();
    assert!(!is_hot_draining_change(&base, &base));
    for mutate in [
        (|profile: &mut Profile| profile.cpu_count += 1) as fn(&mut Profile),
        |profile: &mut Profile| profile.memory_mb += 1_024,
        |profile: &mut Profile| profile.storage_mb += 1_024,
        |profile: &mut Profile| profile.network = flanforge_core::NetworkMode::Default,
        |profile: &mut Profile| {
            profile.template =
                VmName::new("other-base").unwrap_or_else(|error| unreachable!("fixture: {error}"));
        },
        |profile: &mut Profile| {
            profile.hot = Some(HotConfig {
                max_jobs: 3,
                ..hot_config()
            });
        },
    ] {
        let mut next = base.clone();
        mutate(&mut next);
        assert!(is_hot_draining_change(&base, &next));
    }
    // A field a running machine can absorb is not a drain.
    let mut retimed = base.clone();
    retimed.job_timeout_seconds += 60;
    assert!(!is_hot_draining_change(&base, &retimed));
}

/// The stale-warm bound is one of the two non-configurable ones, so it has to
/// compare the machine with what the profile resolves to *now*. Comparing the
/// record with itself is always equal, which is a bound that never fires.
#[tokio::test]
async fn a_machine_built_from_a_generation_the_profile_no_longer_promotes_drains() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::hot_config(directory.path().to_path_buf());
    let manager = manager_with(directory.path(), config, Arc::new(GatedWorker)).await;
    let mut superseded = guest("ci-hot-project-0123456789ab", 1);
    superseded.warm_generation = Some(3);
    save_hot(&manager, &superseded).await;

    manager.ensure_hot_bounded().await;

    assert!(
        manager.hot_list().await.is_empty(),
        "a machine built from a generation the profile does not promote stayed claimable"
    );
}

/// The eviction runs *before* `ensure_capacity` folds, and that ordering is
/// load-bearing rather than tidy: on a one-slot host with a retained machine
/// holding the slot, a deferred teardown deadlocks — the allocation cannot be
/// admitted until the slot is free, and the slot is not freed until the
/// allocation completes.
#[tokio::test]
async fn an_evict_request_frees_the_only_slot_before_capacity_folds() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = Arc::new(capped(directory.path(), 1));
    let manager = manager_with(directory.path(), config, Arc::new(HostedWorker::default())).await;
    save_hot(&manager, &guest(POOLED, 1)).await;

    let created = manager
        .create(
            test_support::request(),
            flanforge_core::RequestOptions {
                hot: flanforge_core::HotRequest::Evict,
                ..flanforge_core::RequestOptions::default()
            },
            &test_support::claims(),
        )
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    assert!(created.is_new);
    assert!(
        manager.hot_list().await.is_empty(),
        "the profile's pool outlived the run that ended its hot session"
    );
    // `"evict"` keeps nothing of its own, so the pool stays empty behind it.
    assert!(created.allocation.hot_lane.is_none());
    assert!(created.allocation.hot_refusal.is_none());
}

/// The kill switch has to drain, not freeze: machines it left standing would
/// keep their capacity charge, and `ensure_hot_yielded` is bounded by the same
/// zero and so could never free them either.
#[tokio::test]
async fn the_global_kill_switch_drains_the_pool_rather_than_freezing_it() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = Arc::new(capped(directory.path(), 0));
    let manager = manager_with(directory.path(), Arc::clone(&config), Arc::new(GatedWorker)).await;
    save_hot(&manager, &guest(POOLED, 1)).await;

    manager.ensure_hot_bounded().await;
    assert!(
        manager.hot_list().await.is_empty(),
        "the kill switch froze the pool instead of draining it"
    );
}

/// A retained machine is a cache, and a cache yields under pressure. Without
/// this, one machine on a one-slot host refuses every allocation until it ages
/// out — and the yield has to survive the cap being lowered underneath it,
/// including to the zero that is the kill switch.
#[tokio::test]
async fn an_idle_machine_yields_its_slot_to_an_allocation_that_needs_it() {
    for max_hot_vms in [0, 1] {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let config = Arc::new(capped(directory.path(), max_hot_vms));
        let manager = manager_with(
            directory.path(),
            Arc::clone(&config),
            Arc::new(HostedWorker::default()),
        )
        .await;
        save_hot(&manager, &guest(POOLED, 1)).await;

        manager
            .ensure_hot_yielded(&config, test_support::size())
            .await;

        assert!(
            manager.hot_list().await.is_empty(),
            "an idle machine kept the only slot under max_hot_vms = {max_hot_vms}"
        );
    }
}

/// A claimed machine never yields: freeing this allocation's slot by failing
/// somebody else's running job is not a trade the pool may make.
#[tokio::test]
async fn a_claimed_machine_never_yields_its_slot() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = Arc::new(capped(directory.path(), 1));
    let manager = manager_with(
        directory.path(),
        Arc::clone(&config),
        Arc::new(HostedWorker::default()),
    )
    .await;
    let mut claimed = guest(POOLED, 1);
    claimed
        .ensure_claimed(AllocationId::new())
        .unwrap_or_else(|error| unreachable!("claim: {error}"));
    save_hot(&manager, &claimed).await;

    manager
        .ensure_hot_yielded(&config, test_support::size())
        .await;

    assert_eq!(
        manager.hot_list().await.len(),
        1,
        "a machine running somebody's job was evicted to admit another"
    );
}

/// A reusing allocation's `vm_name` is a fresh name that never becomes a
/// machine, so the cap must count the machine its origin names — which the
/// record snapshot already counts — rather than charging the host twice.
#[test]
fn a_reusing_allocation_does_not_charge_the_pool_a_second_slot() {
    let held = "ci-hot-project-0123456789ab";
    let mut recorded = guest(held, 1);
    recorded
        .ensure_claimed(AllocationId::new())
        .unwrap_or_else(|error| unreachable!("claim: {error}"));

    let mut reuser = hot_allocation(held);
    reuser.set_hot(Some(HotLane::Protected), reuser.origin.clone(), None, None);
    let context = context_with(2);

    assert!(
        matches!(
            super::claim::plan_hot_claim(
                &entries(vec![reuser]),
                &[recorded],
                &context,
                AllocationId::new(),
            ),
            Some(super::HotClaim::Retain)
        ),
        "a host holding one machine refused a second because it charged the reuser too"
    );
}

/// `Provisioning` and `Recycling` have no edge to `Draining`, so a drain that
/// lands on one has to leave something behind: without it an `hot: "evict"`
/// racing a recycle rejoins the pool, and a retention whose compensating remove
/// failed holds a slot until the daemon restarts.
#[tokio::test]
async fn a_drain_that_lands_mid_operation_is_stamped_and_retried() {
    for state in [HotState::Provisioning, HotState::Recycling] {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let config = test_support::hot_config(directory.path().to_path_buf());
        let manager = manager_with(directory.path(), config, Arc::new(GatedWorker)).await;
        let mut mid_operation = guest("ci-hot-project-0123456789ab", 1);
        mid_operation.state = state;
        save_hot(&manager, &mid_operation).await;

        manager
            .ensure_hot_evicted_for(
                &test_support::profile_name(),
                HotDrainReason::EvictRequested,
            )
            .await;
        let pending = manager.hot_list().await;
        assert_eq!(
            pending.first().and_then(|status| status.drain_reason),
            Some(HotDrainReason::EvictRequested),
            "a {state:?} record kept no record of the drain that reached it"
        );

        manager.ensure_hot_bounded().await;
        assert!(
            manager.hot_list().await.is_empty(),
            "a stamped {state:?} record was never retried"
        );
    }
}

/// `max_idle` and `idle_ttl_seconds` are different causes with different fixes,
/// so an operator reading `hot list` must be able to tell them apart.
#[tokio::test]
async fn an_idle_overshoot_names_max_idle_rather_than_the_ttl() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = test_support::hot_config(directory.path().to_path_buf());
    let manager = manager_with(directory.path(), config, Arc::new(UnevictableWorker)).await;
    let mut stalest = guest("ci-hot-project-0123456789ab", 1);
    stalest.updated_at_unix -= 60;
    save_hot(&manager, &stalest).await;
    save_hot(&manager, &guest("ci-hot-project-ba9876543210", 1)).await;

    manager.ensure_hot_bounded().await;

    let retired = manager
        .hot_list()
        .await
        .into_iter()
        .find(|status| status.vm_name.as_str() == "ci-hot-project-0123456789ab")
        .unwrap_or_else(|| unreachable!("the stalest record is kept while its eviction fails"));
    assert_eq!(retired.drain_reason, Some(HotDrainReason::MaxIdle));
    assert_eq!(retired.state, HotState::Draining);
}

use std::sync::Arc;

use async_trait::async_trait;
use flanforge_wire::RuntimeCapabilities;

use super::super::{
    AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle, HostMachine, WorkerError,
};

/// The one machine every pool fixture here holds.
const POOLED: &str = "ci-hot-project-0123456789ab";

/// A hot configuration whose host has exactly one slot, so an idle pool machine
/// and an incoming allocation cannot both have it.
fn capped(state_dir: &std::path::Path, max_hot_vms: u8) -> flanforge_core::Config {
    let mut config = (*test_support::hot_config(state_dir.to_path_buf())).clone();
    config.runtime.max_hot_vms = max_hot_vms;
    config.runtime.max_running_vms = 1;
    config
}

/// A hot record on disk, seeded through the manager's own store so the tests
/// read the same durable state the daemon does.
async fn save_hot(manager: &AllocationManager, guest: &HotGuest) {
    manager
        .inner
        .hot
        .save(guest)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
}

/// The fixture profile's live policy, with the global cap under test.
fn context_with(max_hot_vms: u8) -> HotContext {
    HotContext {
        hot: hot_config(),
        name: test_support::profile_name(),
        lane: HotLane::Protected,
        fingerprint: None,
        warm_generation: None,
        max_hot_vms,
        age_seconds: None,
    }
}

/// One tempdir's worth of durable dependencies, so every test here seeds the
/// same state directory the same way.
async fn manager_with(
    directory: &std::path::Path,
    config: Arc<flanforge_core::Config>,
    worker: Arc<dyn AllocationWorker>,
) -> AllocationManager {
    AllocationManager::new(
        ConfigHandle::new(config),
        Arc::new(
            SqliteAllocationStore::open(&directory.join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::new(
            SqliteWarmImageStore::open(&directory.join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        Arc::new(
            SqliteHotGuestStore::open(&directory.join("state.db"))
                .await
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        worker,
    )
}

fn source() -> CloneSource {
    CloneSource {
        name: VmName::new("flanforge-base")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        kind: CloneKind::Template,
        base_fingerprint: None,
        fallback_reason: None,
    }
}

/// A backend that keeps every machine it is handed and passes every gate.
#[derive(Debug)]
struct GatedWorker;

#[async_trait]
impl AllocationWorker for GatedWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn ensure_hot_retained(
        &self,
        _request: super::super::HotRetainRequest<'_>,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn ensure_hot_reset(
        &self,
        _guest: &HotGuest,
        _reset: super::super::HotReset,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn is_hot_live(&self, _guest: &HotGuest) -> bool {
        true
    }

    async fn ensure_hot_evicted(
        &self,
        _guest: &HotGuest,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::tart()
    }
}

/// A backend that reports the pool's machine on the host listing until it is
/// destroyed: what the capacity yield and the evict-first ordering actually
/// fold against. A machine still listed after its record is gone is a foreign
/// one, so the listing has to follow the eviction.
#[derive(Debug, Default)]
struct HostedWorker {
    is_evicted: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AllocationWorker for HostedWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        if self.is_evicted.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        Ok(vec![HostMachine {
            name: POOLED.to_owned(),
            state: super::super::MachineState::Running,
            age_seconds: Some(10),
            size: None,
            ownership: super::super::MachineOwnership::Owned,
        }])
    }

    async fn ensure_hot_evicted(
        &self,
        _guest: &HotGuest,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        self.is_evicted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::tart()
    }
}

/// A backend that cannot destroy a pool machine, which is what leaves a drained
/// record on disk for its reason to be read from.
#[derive(Debug)]
struct UnevictableWorker;

#[async_trait]
impl AllocationWorker for UnevictableWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::tart()
    }
}

/// A backend that declares no hot capability, the way every backend did before
/// one could hold a machine open.
#[derive(Debug)]
struct ColdOnlyWorker;

#[async_trait]
impl AllocationWorker for ColdOnlyWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(Vec::new())
    }

    /// The Tart set as it stood before a backend could hold a machine open.
    /// It is still canonical, so it decodes, and it declares no hot support.
    fn capabilities(&self) -> RuntimeCapabilities {
        serde_json::from_str(r#"["host_copy_runner","warm_images"]"#)
            .unwrap_or_else(|error| unreachable!("historical capability set: {error}"))
    }
}
