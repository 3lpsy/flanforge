mod allocation;
mod warm;

use flanforge_libvirt_wire::{
    HelperFailureCode, HelperGuestSize, HelperMachine, HelperMachineOwnership, HelperMachineState,
    HelperReply, HelperRequest,
};
use flanforge_manager::{HostMachine, MachineOwnership, MachineState};
use virt::{connect::Connect, error::clear_error_callback};

use crate::RuntimeError;

use allocation::allocation;
use warm::warm;

use super::{inventory, storage};

pub(super) fn execute(request: HelperRequest) -> HelperReply {
    clear_error_callback();
    let mut connection = match Connect::open(Some(&config(&request).uri)) {
        Ok(connection) => connection,
        Err(error) => {
            return HelperReply::error(
                HelperFailureCode::Unavailable,
                RuntimeError::libvirt("connection", error),
            );
        }
    };
    let reply = dispatch(&connection, request)
        .unwrap_or_else(|error| HelperReply::Error(error.helper_failure()));
    match connection.close() {
        Ok(_) => reply,
        Err(_error) if matches!(reply, HelperReply::Error(_)) => reply,
        Err(error) => HelperReply::error(
            HelperFailureCode::Libvirt,
            RuntimeError::libvirt("connection close", error),
        ),
    }
}

fn dispatch(connection: &Connect, request: HelperRequest) -> Result<HelperReply, RuntimeError> {
    match request {
        HelperRequest::Probe { config } => probe(connection, &config),
        HelperRequest::Inventory { config } => Ok(HelperReply::Inventory(
            inventory::list(connection, &config)?
                .into_iter()
                .map(helper_machine)
                .collect(),
        )),
        request => allocation(connection, request),
    }
}

pub(super) fn operation(request: &HelperRequest) -> &'static str {
    match request {
        HelperRequest::Probe { .. } => "probe",
        HelperRequest::Inventory { .. } => "inventory",
        HelperRequest::CheckSource { .. } => "check-source",
        HelperRequest::Create { .. } => "create",
        HelperRequest::Define { .. } => "define",
        HelperRequest::Start { .. } => "start",
        HelperRequest::Address { .. } => "address",
        HelperRequest::AgentProbe { .. } => "agent-probe",
        HelperRequest::AgentExec { .. } => "agent-exec",
        HelperRequest::AgentExecStatus { .. } => "agent-exec-status",
        HelperRequest::Cleanup { .. } => "cleanup",
        HelperRequest::Import { .. } => "import",
        HelperRequest::DeleteVolume { .. } => "delete-volume",
        HelperRequest::WarmQuiesce { .. } => "warm-quiesce",
        HelperRequest::WarmCapture { .. } => "warm-capture",
        HelperRequest::WarmVerify { .. } => "warm-verify",
        HelperRequest::WarmRetire { .. } => "warm-retire",
    }
}

fn probe(
    connection: &Connect,
    config: &flanforge_libvirt_wire::HelperConfig,
) -> Result<HelperReply, RuntimeError> {
    storage::probe(connection, config).map(|()| HelperReply::Unit)
}

fn config(request: &HelperRequest) -> &flanforge_libvirt_wire::HelperConfig {
    match request {
        HelperRequest::Probe { config }
        | HelperRequest::Inventory { config }
        | HelperRequest::CheckSource { config, .. }
        | HelperRequest::WarmQuiesce { config, .. }
        | HelperRequest::WarmCapture { config, .. }
        | HelperRequest::WarmVerify { config, .. }
        | HelperRequest::WarmRetire { config, .. }
        | HelperRequest::Create { config, .. }
        | HelperRequest::Define { config, .. }
        | HelperRequest::Start { config, .. }
        | HelperRequest::Address { config, .. }
        | HelperRequest::AgentProbe { config, .. }
        | HelperRequest::AgentExec { config, .. }
        | HelperRequest::AgentExecStatus { config, .. }
        | HelperRequest::Cleanup { config, .. }
        | HelperRequest::Import { config, .. }
        | HelperRequest::DeleteVolume { config, .. } => config,
    }
}

fn helper_machine(machine: HostMachine) -> HelperMachine {
    HelperMachine {
        name: machine.name,
        state: match machine.state {
            MachineState::Running => HelperMachineState::Running,
            MachineState::Stopped => HelperMachineState::Stopped,
            MachineState::Other => HelperMachineState::Other,
        },
        age_seconds: machine.age_seconds,
        size: machine.size.map(|size| HelperGuestSize {
            cpu_count: size.cpu_count,
            memory_mb: size.memory_mb,
            storage_mb: size.storage_mb,
        }),
        ownership: match machine.ownership {
            MachineOwnership::Owned => HelperMachineOwnership::Owned,
            MachineOwnership::Foreign => HelperMachineOwnership::Foreign,
            MachineOwnership::Unknown => HelperMachineOwnership::Unknown,
        },
    }
}
