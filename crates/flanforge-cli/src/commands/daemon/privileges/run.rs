use std::path::Path;

use anyhow::Result;

use crate::cli::DaemonPrivArgs;

use super::{
    elevate,
    gates::{firewall_gate, tcc_gates},
    plan::{Plan, ensure_arguments_consistent},
    probe::{CHECK_TIMEOUT, PROMPT_TIMEOUT, run_probes},
    report::{ensure_satisfied, print_field},
    target,
};

/// Reports the three macOS gates that key on the service binary's path, grants
/// the one a command can grant, and provokes the two only a human can answer.
///
/// # Errors
///
/// Returns an error when a gate in scope is unsatisfied, so the command is
/// usable as a pre-flight.
pub(crate) async fn priv_gates(config_path: &Path, arguments: DaemonPrivArgs) -> Result<()> {
    ensure_arguments_consistent(&arguments)?;
    if let Some(binary) = &arguments.elevated {
        return elevate::apply_elevated(binary).await;
    }
    if arguments.probe {
        return print_probes(config_path, arguments.prompt).await;
    }

    let plan = Plan::from_arguments(&arguments);
    let target = target::resolve()?;
    print_field("binary", &target.binary.display().to_string());
    if let Some(note) = &target.note {
        print_field("running", note);
    }
    if plan.is_prompting {
        println!("answer any macOS prompt now; it is attributed to the binary above");
    }

    let mut gates = vec![firewall_gate(config_path, &target, plan).await];
    gates.extend(tcc_gates(config_path, &target, plan).await);
    for gate in &gates {
        gate.print();
    }
    ensure_satisfied(&gates, plan)
}

/// The hidden probe mode: one machine-readable line per gate, for the parent
/// process that delegated the access to this binary.
async fn print_probes(config_path: &Path, is_prompting: bool) -> Result<()> {
    let bound = if is_prompting {
        PROMPT_TIMEOUT
    } else {
        CHECK_TIMEOUT
    };
    let probes = run_probes(config_path, bound).await;
    println!("volume={}", probes.volume.token());
    println!("network={}", probes.network.token());
    Ok(())
}
