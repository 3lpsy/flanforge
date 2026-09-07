use flanforge_core::GuestSize;
use flanforge_libvirt_wire::{
    HelperMachine, HelperMachineOwnership, HelperMachineState, HelperReply,
};
use flanforge_manager::{HostMachine, MachineOwnership, MachineState};

use crate::RuntimeError;

pub(super) fn expect_unit(reply: HelperReply) -> Result<(), RuntimeError> {
    match reply {
        HelperReply::Unit => Ok(()),
        reply => unexpected(&reply),
    }
}

pub(super) fn unexpected<T>(reply: &HelperReply) -> Result<T, RuntimeError> {
    Err(RuntimeError::helper(format!(
        "libvirt helper returned an unexpected {} reply",
        reply_kind(reply)
    )))
}

fn reply_kind(reply: &HelperReply) -> &'static str {
    match reply {
        HelperReply::Unit => "unit",
        HelperReply::Inventory(_) => "inventory",
        HelperReply::Manifest(_) => "manifest",
        HelperReply::Address(_) => "address",
        HelperReply::Published(_) => "published-base",
        HelperReply::Warm(_) => "warm-pointer",
        HelperReply::Retired(_) => "retired",
        HelperReply::AgentProbe(_) => "agent-probe",
        HelperReply::AgentStarted(_) => "agent-started",
        HelperReply::AgentOutcome(_) => "agent-outcome",
        HelperReply::Error(_) => "error",
    }
}

pub(super) fn host_machine(machine: HelperMachine) -> HostMachine {
    let size = machine.size.map(|size| GuestSize {
        cpu_count: size.cpu_count,
        memory_mb: size.memory_mb,
        storage_mb: size.storage_mb,
    });
    HostMachine {
        name: machine.name,
        state: match machine.state {
            HelperMachineState::Running => MachineState::Running,
            HelperMachineState::Stopped => MachineState::Stopped,
            HelperMachineState::Other => MachineState::Other,
        },
        age_seconds: machine.age_seconds,
        size,
        ownership: match machine.ownership {
            HelperMachineOwnership::Owned => MachineOwnership::Owned,
            HelperMachineOwnership::Foreign => MachineOwnership::Foreign,
            HelperMachineOwnership::Unknown => MachineOwnership::Unknown,
        },
    }
}
