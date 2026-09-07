use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use flanforge_core::{Allocation, RuntimeBackendConfig, RuntimeConfig, TartConfig, VmName};
use serde::Deserialize;
use tokio::process::Command;
use validator::Validate;

use flanforge_manager::{HostMachine, MachineOwnership, MachineState, WorkerError};

/// A wedged Tart must surface as a bounded failure, never as a hung phase.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// Tart's own default library, relative to the service account's home.
const DEFAULT_LIBRARY_DIRECTORY: &str = ".tart";

#[derive(Clone, Debug)]
pub(crate) struct TartClient {
    pub(super) config: RuntimeConfig,
    tart: TartConfig,
    /// Resolved once, because `runtime.backend.home` and the process
    /// environment are both captured at startup. None when nothing could be
    /// derived, which leaves every VM unageable rather than wrongly aged.
    library: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Machine {
    pub(crate) name: String,
    pub(crate) state: String,
}

impl Machine {
    /// Exactly `stopped`: any other spelling is an unknown state, not a
    /// stopped one.
    pub(super) fn is_stopped(&self) -> bool {
        self.state == "stopped"
    }

    fn machine_state(&self) -> MachineState {
        if self.state.eq_ignore_ascii_case("running") {
            MachineState::Running
        } else if self.is_stopped() {
            MachineState::Stopped
        } else {
            MachineState::Other
        }
    }
}

impl TartClient {
    pub(crate) fn new(config: RuntimeConfig) -> Self {
        let RuntimeBackendConfig::Tart(tart) = config.backend.clone() else {
            unreachable!("Tart client is composed only for the Tart backend")
        };
        let library = library_home(
            tart.home.as_deref(),
            std::env::var_os("TART_HOME").map(PathBuf::from).as_deref(),
            flanforge_paths::service_home().ok().as_deref(),
        );
        Self {
            config,
            tart,
            library,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_library(config: RuntimeConfig, library: Option<PathBuf>) -> Self {
        Self {
            library,
            ..Self::new(config)
        }
    }

    /// The library the `tart` child reads, for the one thing that needs a path
    /// rather than a subprocess: a clone's directory mtime.
    pub(crate) fn library(&self) -> Option<&Path> {
        self.library.as_deref()
    }

    /// Every machine the host reports, with the age the reaper needs.
    pub(crate) async fn host_machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        let mut machines = Vec::new();
        for machine in self.list().await? {
            let age_seconds = self.machine_age_seconds(&machine.name).await;
            machines.push(HostMachine {
                name: machine.name.clone(),
                state: machine.machine_state(),
                age_seconds,
                size: None,
                ownership: if self.is_owned_name(&machine.name) {
                    MachineOwnership::Owned
                } else {
                    MachineOwnership::Foreign
                },
            });
        }
        Ok(machines)
    }

    pub(super) async fn remove_machine(
        &self,
        allocation: &Allocation,
        state: &str,
    ) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        self.remove_named(&allocation.vm_name, state).await
    }

    /// Deletes a listed machine by name; every caller proves its authority
    /// before reaching here.
    pub(crate) async fn remove_named(&self, name: &VmName, state: &str) -> Result<(), WorkerError> {
        // The listed state is a stale snapshot, so a failed stop must never
        // withhold the delete that upholds the one-job VM lifetime.
        if state.eq_ignore_ascii_case("running")
            && self.run_checked(&["stop", name.as_str()]).await.is_err()
        {
            tracing::warn!(vm_name = %name, "Tart VM stop failed; deleting anyway");
        }
        if let Err(error) = self.run_checked(&["delete", name.as_str()]).await {
            if self.is_absent(name.as_str()).await {
                tracing::debug!(vm_name = %name, "Tart VM was already deleted");
                return Ok(());
            }
            return Err(error);
        }
        tracing::info!(vm_name = %name, "Tart VM deleted");
        Ok(())
    }

    pub(crate) async fn is_absent(&self, name: &str) -> bool {
        self.list()
            .await
            .is_ok_and(|machines| !machines.iter().any(|machine| machine.name == name))
    }

    pub(crate) async fn list(&self) -> Result<Vec<Machine>, WorkerError> {
        let mut attempt = 0;
        let output = loop {
            attempt += 1;
            let invocation = self.command().args(["list", "--format", "json"]).output();
            match tokio::time::timeout(LIST_TIMEOUT, invocation).await {
                Ok(Ok(output)) => break output,
                Ok(Err(error)) if attempt < 2 => {
                    tracing::debug!(kind = ?error.kind(), "retrying Tart machine inspection after invocation failure");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok(Err(_)) => return Err(WorkerError::new("cannot inspect Tart VMs")),
                Err(_) => return Err(WorkerError::new("Tart machine inspection timed out")),
            }
        };
        if !output.status.success() || output.stdout.len() > 1_048_576 {
            return Err(WorkerError::new("cannot inspect Tart VMs"));
        }
        let machines: Vec<Machine> = serde_json::from_slice(&output.stdout)
            .map_err(|_| WorkerError::new("Tart returned invalid VM state"))?;
        tracing::trace!(machines = machines.len(), "Tart machine list decoded");
        Ok(machines)
    }

    pub(super) async fn run_checked(&self, arguments: &[&str]) -> Result<(), WorkerError> {
        let operation = arguments.first().copied().unwrap_or("unknown").to_owned();
        tracing::debug!(%operation, "invoking Tart operation");
        let status = self
            .command()
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| WorkerError::new("cannot invoke Tart"))?;
        if status.success() {
            tracing::debug!(%operation, "Tart operation completed");
            Ok(())
        } else {
            tracing::warn!(%operation, exit_code = status.code(), "Tart operation failed");
            Err(WorkerError::new("Tart operation failed"))
        }
    }

    pub(super) fn ensure_owned(&self, allocation: &Allocation) -> Result<(), WorkerError> {
        allocation
            .validate()
            .map_err(|_| WorkerError::new("allocation is structurally invalid"))?;
        if self.is_owned(&allocation.vm_name) {
            Ok(())
        } else {
            tracing::error!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "refused Tart operation outside configured prefix");
            Err(WorkerError::new("refusing to modify an unowned Tart VM"))
        }
    }

    /// The name-addressed half of `ensure_owned`, for a pool machine that
    /// outlived the allocation which cloned it and has only its name left.
    pub(crate) fn ensure_owned_name(&self, name: &VmName) -> Result<(), WorkerError> {
        if self.is_owned(name) {
            Ok(())
        } else {
            tracing::error!(vm_name = %name, "refused Tart operation outside configured prefix");
            Err(WorkerError::new("refusing to modify an unowned Tart VM"))
        }
    }

    pub(crate) fn is_owned(&self, name: &VmName) -> bool {
        self.is_owned_name(name.as_str())
    }

    /// Tart exposes no per-VM metadata, so the configured prefix is the only
    /// ownership evidence there is — the same authority that gates every
    /// delete in this client.
    pub(crate) fn is_owned_name(&self, name: &str) -> bool {
        name.starts_with(self.config.vm_prefix.as_str())
    }

    pub(super) fn command(&self) -> Command {
        let mut command = Command::new(&self.tart.path);
        command.kill_on_drop(true);
        // Only ever the configured value: the child resolves its own default
        // exactly as `library_home` does, and forcing a derived path on it
        // would turn a wrong derivation into a wrong library.
        if let Some(home) = &self.tart.home {
            command.env("TART_HOME", home);
        }
        command
    }
}

/// The library the `tart` child will read, resolved the way Tart resolves it:
/// the configured path, else the inherited `TART_HOME`, else the service
/// account's `~/.tart`. Deriving rather than requiring is what lets the sweep
/// age a candidate without `runtime.backend.home` being set.
///
/// Kept free of the process environment so the order is directly testable.
/// None where nothing is left to derive from, and equally where `TART_HOME` is
/// set to something unusable: the child would still honour that value, so
/// falling through to `~/.tart` would name a library Tart is not reading.
pub(crate) fn library_home(
    configured: Option<&Path>,
    tart_home: Option<&Path>,
    service_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(configured) = configured {
        return Some(configured.to_path_buf());
    }
    if let Some(tart_home) = tart_home {
        return tart_home.is_absolute().then(|| tart_home.to_path_buf());
    }
    service_home.map(|home| home.join(DEFAULT_LIBRARY_DIRECTORY))
}
