use anyhow::{Result, bail};

use super::plan::{Gated, Plan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GateState {
    Satisfied,
    Missing,
    Unknown,
    NotApplicable,
}

impl GateState {
    pub(super) fn is_satisfied(self) -> bool {
        matches!(self, Self::Satisfied | Self::NotApplicable)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
            Self::NotApplicable => "not applicable",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Gate {
    pub(super) gated: Gated,
    pub(super) state: GateState,
    pub(super) detail: String,
    pub(super) fixes: Vec<String>,
}

impl Gate {
    pub(super) fn new(gated: Gated, state: GateState, detail: impl Into<String>) -> Self {
        Self {
            gated,
            state,
            detail: detail.into(),
            fixes: Vec::new(),
        }
    }

    pub(super) fn with_fixes<I, S>(mut self, fixes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if !self.state.is_satisfied() {
            self.fixes = fixes.into_iter().map(Into::into).collect();
        }
        self
    }

    pub(super) fn print(&self) {
        println!(
            "{:<12}{} ({})",
            format!("{}:", self.gated.label()),
            self.state.label(),
            self.detail
        );
        for fix in &self.fixes {
            println!("{:<12}fix: {fix}", "");
        }
    }
}

impl Gated {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Firewall => "firewall",
            Self::RemovableVolume => "volume",
            Self::LocalNetwork => "network",
        }
    }
}

pub(super) fn print_field(name: &str, value: &str) {
    println!("{:<12}{value}", format!("{name}:"));
}

/// Non-zero exit for anything in scope that is not satisfied, so the command
/// works as a script's pre-flight.
pub(super) fn ensure_satisfied(gates: &[Gate], plan: Plan) -> Result<()> {
    let unsatisfied: Vec<&str> = gates
        .iter()
        .filter(|gate| plan.is_in_scope(gate.gated) && !gate.state.is_satisfied())
        .map(|gate| gate.gated.label())
        .collect();
    if unsatisfied.is_empty() {
        return Ok(());
    }
    bail!(
        "unsatisfied host permission gate(s): {}",
        unsatisfied.join(", ")
    );
}
