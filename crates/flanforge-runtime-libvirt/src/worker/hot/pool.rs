use flanforge_core::VmName;
use tokio::sync::Mutex;

use std::collections::BTreeSet;

/// Every domain the pool has claimed.
///
/// Unlike Tart there is no child process to hold: a libvirt domain is not a
/// child of `flanforged`, so it outlives both the worker that built it and a
/// daemon restart for free. The set exists only so `cleanup` can ask whether
/// the pool kept this machine — the same question Tart's map of `tart run`
/// children answers.
#[derive(Debug, Default)]
pub(crate) struct HotPool {
    claimed: Mutex<BTreeSet<String>>,
}

impl HotPool {
    /// Takes the domain into the pool. Idempotent, because a machine reaches
    /// the gate from retention, from a release, and from restart adoption.
    pub(crate) async fn ensure_claimed(&self, name: &VmName) {
        if self.claimed.lock().await.insert(name.as_str().to_owned()) {
            tracing::debug!(vm_name = %name, "the pool claimed a finished allocation's guest");
        }
    }

    pub(crate) async fn is_claimed(&self, name: &VmName) -> bool {
        self.claimed.lock().await.contains(name.as_str())
    }

    /// Withdraws the claim, leaving the domain untouched: whoever withdrew it
    /// is what destroys it.
    pub(crate) async fn release(&self, name: &VmName) {
        if self.claimed.lock().await.remove(name.as_str()) {
            tracing::debug!(vm_name = %name, "the pool released its claim on a guest");
        }
    }
}
