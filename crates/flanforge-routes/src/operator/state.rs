use std::net::SocketAddr;

use flanforge_manager::AllocationManager;

/// The host-only surface. It carries no OIDC verifier, because a workflow
/// identity means nothing here; what it does carry is the credential only a
/// process that can read the state directory can present.
#[derive(Clone)]
pub struct OperatorState {
    pub(super) manager: AllocationManager,
    pub(super) listen: SocketAddr,
    pub(super) token: String,
}

impl std::fmt::Debug for OperatorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperatorState")
            .field("listen", &self.listen)
            .finish_non_exhaustive()
    }
}

impl OperatorState {
    #[must_use]
    pub fn new(manager: AllocationManager, listen: SocketAddr, token: String) -> Self {
        Self {
            manager,
            listen,
            token,
        }
    }
}
