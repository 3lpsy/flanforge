use std::{sync::atomic::Ordering, time::Duration};

use flanforge_core::{Allocation, AllocationId, AllocationState, RepositoryName, VmName};

use super::{AllocationManager, CleanupBudget};

/// Fraction of the grace held back from teardown, so the terminal record and
/// this report still land inside it.
const BOOKKEEPING_RESERVE: u32 = 8;

/// Why a shutdown could not finish tearing one allocation down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeakReason {
    /// Teardown ran but did not complete inside the grace.
    CleanupFailed,
    /// The allocation never reached a terminal state before the grace expired.
    NotTerminal,
}

impl LeakReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CleanupFailed => "cleanup did not complete within the shutdown grace",
            Self::NotTerminal => {
                "allocation did not reach a terminal state within the shutdown grace"
            }
        }
    }
}

/// One allocation a shutdown left behind, carrying what an operator needs to
/// find the guest and the runner registration by hand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocationLeak {
    pub id: AllocationId,
    /// The state the allocation was left in.
    pub state: AllocationState,
    pub repository: RepositoryName,
    pub vm_name: VmName,
    /// Only a created guest can still exist on the host.
    pub is_vm_created: bool,
    /// Present once the runtime registered the ephemeral runner.
    pub runner_id: Option<i64>,
    pub reason: LeakReason,
}

impl AllocationLeak {
    fn new(allocation: &Allocation, reason: LeakReason) -> Self {
        Self {
            id: allocation.id,
            state: allocation.state,
            repository: allocation.request.repository.clone(),
            vm_name: allocation.vm_name.clone(),
            is_vm_created: allocation.vm_created,
            runner_id: allocation.runner_id,
            reason,
        }
    }

    /// The ephemeral runner name the runtime registers, once it registered one.
    #[must_use]
    pub fn runner_name(&self) -> Option<String> {
        self.runner_id.map(|_| format!("flanforged-{}", self.id))
    }
}

/// What one shutdown could not finish, and how much of the grace it spent
/// trying.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShutdownReport {
    leaked: Vec<AllocationLeak>,
    grace: Duration,
    spent: Duration,
}

impl ShutdownReport {
    /// Did every cancelled allocation tear down inside the grace?
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.leaked.is_empty()
    }

    #[must_use]
    pub fn leaked(&self) -> &[AllocationLeak] {
        &self.leaked
    }

    #[must_use]
    pub const fn grace(&self) -> Duration {
        self.grace
    }

    /// How much of the grace the shutdown actually spent, so it is obvious
    /// whether a larger `shutdown_grace_seconds` would have helped.
    #[must_use]
    pub const fn spent(&self) -> Duration {
        self.spent
    }

    /// One event per leaked allocation, so the log is greppable by allocation
    /// id and by VM name rather than being one blob.
    fn report(&self) {
        for leak in &self.leaked {
            tracing::error!(
                allocation_id = %leak.id,
                state = ?leak.state,
                repository = %leak.repository,
                vm_name = %leak.vm_name,
                vm_created = leak.is_vm_created,
                runner_name = leak.runner_name().as_deref().unwrap_or("none"),
                runner_id = leak.runner_id,
                reason = leak.reason.as_str(),
                grace_ms = self.grace.as_millis(),
                spent_ms = self.spent.as_millis(),
                "shutdown left an allocation behind; the next daemon start reaps it, a manual teardown otherwise"
            );
        }
    }
}

impl AllocationManager {
    /// Cancels active allocations, waits out their teardown, then reports
    /// whatever the grace did not cover.
    pub async fn shutdown(&self, grace: Duration) -> ShutdownReport {
        let started = tokio::time::Instant::now();
        let mut awaited = {
            let entries = self.inner.entries.lock().await;
            self.inner.is_closing.store(true, Ordering::Release);
            // Teardown gets the grace less a bookkeeping reserve, so the
            // terminal record and this report still land inside it.
            let _ = self
                .inner
                .teardown_deadline
                .set(started + grace - grace / BOOKKEEPING_RESERVE);
            entries
                .values()
                .filter(|entry| {
                    !entry.allocation.state.is_terminal()
                        && (entry.is_supervised || entry.is_terminalizing)
                })
                .map(|entry| {
                    entry.cancellation.cancel();
                    (entry.allocation.id, entry.sender.subscribe())
                })
                .collect::<Vec<_>>()
        };
        tracing::info!(
            active_allocations = awaited.len(),
            grace_ms = grace.as_millis(),
            "allocation manager shutdown started"
        );
        let _ = tokio::time::timeout(grace, async {
            for (_, receiver) in &mut awaited {
                while !receiver.borrow_and_update().state.is_terminal() {
                    if receiver.changed().await.is_err() {
                        break;
                    }
                }
            }
        })
        .await;
        let report = ShutdownReport {
            leaked: self.collect_leaks(awaited.iter().map(|(id, _)| *id)).await,
            grace,
            spent: started.elapsed(),
        };
        for leak in &report.leaked {
            self.emit(
                flanforge_store::Event::new(flanforge_store::EventKind::AllocationLeaked)
                    .with_allocation(leak.id)
                    .with_vm_name(leak.vm_name.clone())
                    .with_payload(&serde_json::json!({
                        "state": leak.state,
                        "is_vm_created": leak.is_vm_created,
                        "runner_id": leak.runner_id,
                    })),
            )
            .await;
        }
        if report.is_clean() {
            tracing::info!(
                spent_ms = report.spent.as_millis(),
                "allocation manager shutdown completed"
            );
        } else {
            report.report();
            tracing::warn!(
                leaked = report.leaked.len(),
                grace_ms = grace.as_millis(),
                spent_ms = report.spent.as_millis(),
                "allocation manager shutdown did not finish teardown"
            );
        }
        report
    }

    /// What is left over: teardown that failed inside the grace, plus anything
    /// still non-terminal once it expired.
    async fn collect_leaks(
        &self,
        awaited: impl Iterator<Item = AllocationId>,
    ) -> Vec<AllocationLeak> {
        let mut leaked = std::mem::take(&mut *self.inner.leaked.lock().await);
        let entries = self.inner.entries.lock().await;
        for id in awaited {
            if leaked.iter().any(|leak| leak.id == id) {
                continue;
            }
            let Some(entry) = entries.get(&id) else {
                continue;
            };
            if entry.allocation.state.is_terminal() {
                continue;
            }
            leaked.push(AllocationLeak::new(
                &entry.allocation,
                LeakReason::NotTerminal,
            ));
        }
        leaked
    }

    /// Records teardown a shutdown could not complete. An ordinary teardown
    /// failure is recovery's business, not a leak the operator is told about
    /// while the daemon is still running.
    pub(super) async fn record_leak(&self, allocation: &Allocation, reason: LeakReason) {
        let mut leaked = self.inner.leaked.lock().await;
        if leaked.iter().any(|leak| leak.id == allocation.id) {
            return;
        }
        leaked.push(AllocationLeak::new(allocation, reason));
    }

    /// Puts one configured allowance under the running shutdown's deadline, if
    /// there is one. Every teardown budget in the daemon is minted here, so a
    /// stop caps a reaper deletion exactly as it caps an allocation's cleanup.
    pub(super) fn teardown_budget(&self, allowance: Duration) -> CleanupBudget {
        let budget = CleanupBudget::allow(allowance);
        self.inner
            .teardown_deadline
            .get()
            .map_or(budget, |deadline| budget.until(*deadline))
    }
}
