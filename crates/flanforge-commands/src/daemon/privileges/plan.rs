use anyhow::{Result, bail};

use flanforge_cli::DaemonPrivArgs;

/// Which gate a report line describes, and what the exit status covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gated {
    Firewall,
    RemovableVolume,
    LocalNetwork,
}

/// What the invocation was asked to do. Reporting is the default: anything
/// that touches host state or raises a prompt is requested explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub(super) is_granting_firewall: bool,
    pub(super) is_prompting: bool,
}

impl Plan {
    pub(super) fn from_arguments(arguments: &DaemonPrivArgs) -> Self {
        Self {
            is_granting_firewall: arguments.grant_firewall,
            is_prompting: arguments.prompt,
        }
    }

    /// No action flags means report only, which is also what `--check` asks for.
    pub(super) fn is_report_only(self) -> bool {
        !self.is_granting_firewall && !self.is_prompting
    }

    /// A report covers every gate; an action run is judged on what it acted on.
    pub(super) fn is_in_scope(self, gate: Gated) -> bool {
        if self.is_report_only() {
            return true;
        }
        match gate {
            Gated::Firewall => self.is_granting_firewall,
            Gated::RemovableVolume | Gated::LocalNetwork => self.is_prompting,
        }
    }
}

/// The hidden arguments exist for the re-executions this command performs, so
/// combining them with an operator-facing action is a mistake, not a mode.
pub(super) fn ensure_arguments_consistent(arguments: &DaemonPrivArgs) -> Result<()> {
    if arguments.elevated.is_some()
        && (arguments.check || arguments.probe || arguments.prompt || arguments.grant_firewall)
    {
        bail!("--elevated applies the root-only fixes and takes no other flag");
    }
    if arguments.probe && arguments.grant_firewall {
        bail!("--probe changes nothing and cannot grant a gate");
    }
    Ok(())
}
