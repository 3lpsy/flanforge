use flanforge_core::GuestChannelKind;
use flanforge_libvirt_wire::PublishedBase;
use flanforge_manager::WorkerError;

use crate::agent::JobAccount;

/// The guest contract a published base declares, read before any domain exists
/// so an unusable pairing costs zero seconds rather than a boot timeout.
#[derive(Clone, Debug)]
pub(crate) struct GuestContract {
    version: Option<u8>,
    is_exec_enabled: Option<bool>,
    account: Option<(String, u32)>,
    privileged: Option<(String, u32)>,
}

/// The contract that first bakes the readiness helper and the exec channel.
const AGENT_CONTRACT: u8 = 2;

impl GuestContract {
    pub(crate) fn read(base: &PublishedBase) -> Self {
        let manifest = base.manifest();
        Self {
            version: manifest.guest_contract_version(),
            is_exec_enabled: manifest.is_guest_exec_enabled(),
            account: manifest
                .guest_job_account()
                .map(|(name, uid)| (name.to_owned(), uid)),
            privileged: manifest
                .guest_privileged_account()
                .map(|(name, uid)| (name.to_owned(), uid)),
        }
    }

    /// Whether the base bakes the readiness helper. A manifest-less base is a
    /// qcow2 built by other means, which is a documented workflow: it is probed
    /// at run time rather than refused here.
    pub(crate) fn is_readiness_gate_baked(&self) -> bool {
        self.version
            .is_some_and(|version| version >= AGENT_CONTRACT)
    }

    /// Refuses a pairing the guest can never satisfy, naming what to rebuild.
    pub(crate) fn ensure_channel_supported(
        &self,
        channel: GuestChannelKind,
        runner_user: &str,
    ) -> Result<(), WorkerError> {
        if channel == GuestChannelKind::Ssh {
            return Ok(());
        }
        match (self.version, self.is_exec_enabled) {
            (None, _) => Err(WorkerError::new(
                "the published base declares no guest contract, so the agent channel cannot be \
                 used with it; rebuild and re-import the base, or drive this backend over SSH",
            )),
            (Some(version), Some(false)) if version < AGENT_CONTRACT => {
                Err(WorkerError::new(format!(
                    "the published base predates the guest agent contract (version {version}); \
                     rebuild and re-import the base"
                )))
            }
            (Some(_), Some(false)) => Err(WorkerError::new(
                "the published base reports guest-exec blocked; rebuild it with the RPC enabled, \
                 or drive this backend over SSH",
            )),
            (Some(_), _) => self.job_account(runner_user).map(|_| ()),
        }
    }

    /// The account the agent's root process drops to, cross-checked against
    /// what the operator configured: a mismatch would silently run every job
    /// somewhere the image never prepared.
    pub(crate) fn job_account(&self, runner_user: &str) -> Result<JobAccount, WorkerError> {
        let Some((name, uid)) = self.account.as_ref() else {
            return Err(WorkerError::new(
                "the published base records no job account, so the agent channel cannot drop out \
                 of root; rebuild and re-import the base",
            ));
        };
        if name != runner_user {
            return Err(WorkerError::new(format!(
                "the base image runs jobs as {name} (uid {uid}), not the configured {runner_user}"
            )));
        }
        JobAccount::new(name.clone(), *uid)
    }

    /// The privileged automation account, cross-checked the same way. Its
    /// absence fails only the work that needs it, never the allocation.
    pub(crate) fn privileged_account(
        &self,
        privileged_user: &str,
    ) -> Result<JobAccount, WorkerError> {
        let Some((name, uid)) = self.privileged.as_ref() else {
            return Err(WorkerError::new(
                "the published base records no privileged account, so root-needing guest work \
                 cannot run; rebuild the base with FLANFORGE_PRIVILEGED_ACCOUNT=true and \
                 re-import it",
            ));
        };
        if name != privileged_user {
            return Err(WorkerError::new(format!(
                "the base image bakes {name} (uid {uid}) as its privileged account, not the \
                 configured {privileged_user}"
            )));
        }
        JobAccount::new(name.clone(), *uid)
    }
}
