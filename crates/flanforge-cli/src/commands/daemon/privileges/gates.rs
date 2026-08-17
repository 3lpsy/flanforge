use std::{path::Path, time::Duration};

use flanforge_config::load_config;
use flanforge_core::Config;

use super::{
    delegate::delegated_probes,
    elevate,
    firewall::{self, AppRule, FirewallState},
    outcome::ProbeOutcome,
    plan::{Gated, Plan},
    probe::{CHECK_TIMEOUT, PROMPT_TIMEOUT, Probes, run_probes},
    report::{Gate, GateState},
    target::Target,
};

const GRANT_HINT: &str = "grantable: flanforged daemon priv --grant-firewall (needs root, so it re-executes through sudo)";
const PROMPT_HINT: &str =
    "run `flanforged daemon priv --prompt` to perform the access now and make macOS ask";
const ATTRIBUTION: &str =
    "only a human can answer it, and macOS attributes the prompt to the binary above";
const VOLUME_PANE: &str =
    "open \"x-apple.systempreferences:com.apple.preference.security?Privacy_RemovableVolume\"";
const NETWORK_PANE: &str =
    "open \"x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork\"";

/// Reads the firewall gate and, when asked, grants it and re-reads to confirm.
pub(super) async fn firewall_gate(config_path: &Path, target: &Target, plan: Plan) -> Gate {
    let mut state = firewall::read_state(&target.binary).await;
    if plan.is_granting_firewall && matches!(&state, Ok(read) if !read.is_satisfied()) {
        if let Err(error) = grant_firewall(config_path, &target.binary).await {
            return Gate::new(
                Gated::Firewall,
                GateState::Missing,
                format!("not granted: {error:#}"),
            )
            .with_fixes(firewall::manual_commands(&target.binary));
        }
        state = firewall::read_state(&target.binary).await;
    }
    match state {
        Err(error) => Gate::new(
            Gated::Firewall,
            GateState::Unknown,
            format!("cannot read socketfilterfw: {error:#}"),
        )
        .with_fixes(firewall::manual_commands(&target.binary)),
        Ok(state) if state.is_satisfied() => {
            Gate::new(Gated::Firewall, GateState::Satisfied, describe(state))
        }
        Ok(state) => Gate::new(Gated::Firewall, GateState::Missing, describe(state)).with_fixes(
            std::iter::once(GRANT_HINT.to_owned()).chain(firewall::manual_commands(&target.binary)),
        ),
    }
}

async fn grant_firewall(config_path: &Path, binary: &Path) -> anyhow::Result<()> {
    if elevate::is_root().await? {
        firewall::ensure_allowed(binary).await
    } else {
        elevate::elevate(config_path, binary).await
    }
}

fn describe(state: FirewallState) -> String {
    let listing = match state.rule {
        Some(AppRule::Allow) => "incoming connections allowed for this path",
        Some(AppRule::Block) => "incoming connections blocked for this path",
        None => "this path is not listed, so inbound packets are dropped silently",
    };
    match state.is_enabled {
        Some(true) => format!("firewall on, {listing}"),
        Some(false) => format!("firewall off, {listing}"),
        None => format!("firewall state unknown, {listing}"),
    }
}

/// Probes both consent gates, performing the access as the installed binary
/// whenever that is not this process.
pub(super) async fn tcc_gates(config_path: &Path, target: &Target, plan: Plan) -> Vec<Gate> {
    let bound = if plan.is_prompting {
        PROMPT_TIMEOUT
    } else {
        CHECK_TIMEOUT
    };
    let probes = match &target.probe_via {
        Some(binary) => delegated_probes(binary, config_path, bound, plan.is_prompting).await,
        None => run_probes(config_path, bound).await,
    };
    let config = load_config(config_path).await.ok();
    vec![
        volume_gate(&probes, bound, target, config.as_ref(), plan),
        network_gate(&probes, bound, target, plan),
    ]
}

fn volume_gate(
    probes: &Probes,
    bound: Duration,
    target: &Target,
    config: Option<&Config>,
    plan: Plan,
) -> Gate {
    let seconds = bound.as_secs();
    let gate = match &probes.volume {
        ProbeOutcome::Allowed => Gate::new(
            Gated::RemovableVolume,
            GateState::Satisfied,
            "the configured Tart library is readable",
        ),
        ProbeOutcome::Denied => Gate::new(
            Gated::RemovableVolume,
            GateState::Missing,
            "reading the configured Tart library was refused",
        ),
        ProbeOutcome::TimedOut => Gate::new(
            Gated::RemovableVolume,
            GateState::Missing,
            format!("the read did not return within {seconds}s, so a consent prompt is unanswered"),
        ),
        ProbeOutcome::Skipped(detail) => {
            Gate::new(Gated::RemovableVolume, GateState::NotApplicable, detail)
        }
        ProbeOutcome::Failed(detail) => {
            Gate::new(Gated::RemovableVolume, GateState::Unknown, detail)
        }
    };
    let tart = config.map_or_else(
        || "the configured runtime.tart_path".to_owned(),
        |config| config.runtime.tart_path.display().to_string(),
    );
    gate.with_fixes(manual_fixes(
        VOLUME_PANE,
        &format!(
            "add {} and {tart} under Files and Folders, Removable Volumes",
            target.binary.display()
        ),
        plan,
    ))
}

fn network_gate(probes: &Probes, bound: Duration, target: &Target, plan: Plan) -> Gate {
    let seconds = bound.as_secs();
    let gate = match &probes.network {
        ProbeOutcome::Allowed => Gate::new(
            Gated::LocalNetwork,
            GateState::Satisfied,
            "a local-network send was accepted",
        ),
        ProbeOutcome::Denied => Gate::new(
            Gated::LocalNetwork,
            GateState::Missing,
            "the local-network send was refused",
        ),
        ProbeOutcome::TimedOut => Gate::new(
            Gated::LocalNetwork,
            GateState::Missing,
            format!("no local-network send completed within {seconds}s"),
        ),
        ProbeOutcome::Skipped(detail) => {
            Gate::new(Gated::LocalNetwork, GateState::NotApplicable, detail)
        }
        ProbeOutcome::Failed(detail) => Gate::new(Gated::LocalNetwork, GateState::Unknown, detail),
    };
    gate.with_fixes(manual_fixes(
        NETWORK_PANE,
        &format!("enable {} under Local Network", target.binary.display()),
        plan,
    ))
}

/// Neither consent gate can be granted from a command line, so every fix line
/// is something the operator does.
fn manual_fixes(pane: &str, entry: &str, plan: Plan) -> Vec<String> {
    let mut fixes = vec![ATTRIBUTION.to_owned(), pane.to_owned(), entry.to_owned()];
    if !plan.is_prompting {
        fixes.push(PROMPT_HINT.to_owned());
    }
    fixes
}
