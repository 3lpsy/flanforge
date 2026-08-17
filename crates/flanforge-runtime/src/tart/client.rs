use std::{path::Path, process::Stdio, time::Duration};

use flanforge_core::{Allocation, RuntimeConfig, VmName};
use serde::Deserialize;
use tokio::process::Command;
use validator::Validate;

use flanforge_manager::{HostMachine, MachineState, WorkerError};

/// A wedged Tart must surface as a bounded failure, never as a hung phase.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub(crate) struct TartClient {
    pub(super) config: RuntimeConfig,
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
        Self { config }
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
            && self.run_checked(["stop", name.as_str()]).await.is_err()
        {
            tracing::warn!(vm_name = %name, "Tart VM stop failed; deleting anyway");
        }
        if let Err(error) = self.run_checked(["delete", name.as_str()]).await {
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

    pub(super) async fn run_checked<const N: usize>(
        &self,
        arguments: [&str; N],
    ) -> Result<(), WorkerError> {
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

    pub(crate) fn is_owned(&self, name: &VmName) -> bool {
        name.as_str().starts_with(self.config.vm_prefix.as_str())
    }

    pub(super) fn command(&self) -> Command {
        let mut command = Command::new(&self.config.tart_path);
        command.kill_on_drop(true);
        if let Some(home) = &self.config.tart_home {
            command.env("TART_HOME", home);
        }
        command
    }

    pub(crate) fn runner_host_path(&self) -> &Path {
        &self.config.forgejo_runner_host_path
    }
}
