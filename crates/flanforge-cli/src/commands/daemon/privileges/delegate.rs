use std::{path::Path, process::Stdio, time::Duration};

use tokio::{process::Command, time::timeout};

use super::{
    outcome::{ProbeOutcome, parse_probe_report},
    probe::Probes,
};

/// Slack over the child's own bound, so the parent reports rather than hangs.
const CHILD_SLACK: Duration = Duration::from_secs(15);

/// Runs the probes as the installed binary, because macOS attributes a consent
/// prompt to the executable performing the access.
pub(super) async fn delegated_probes(
    binary: &Path,
    config_path: &Path,
    bound: Duration,
    is_prompting: bool,
) -> Probes {
    match run(binary, config_path, bound, is_prompting).await {
        Ok(report) => Probes {
            volume: outcome(&report, "volume"),
            network: outcome(&report, "network"),
        },
        Err(detail) => Probes {
            volume: ProbeOutcome::Failed(detail.clone()),
            network: ProbeOutcome::Failed(detail),
        },
    }
}

async fn run(
    binary: &Path,
    config_path: &Path,
    bound: Duration,
    is_prompting: bool,
) -> Result<String, String> {
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config_path)
        .arg("daemon")
        .arg("priv")
        .arg("--probe");
    if is_prompting {
        command.arg("--prompt");
    }
    let output = timeout(
        bound + CHILD_SLACK,
        command.stdin(Stdio::null()).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| format!("{} did not answer in time", binary.display()))?
    .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!("{} exited unsuccessfully", binary.display()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn outcome(report: &str, gate: &str) -> ProbeOutcome {
    parse_probe_report(report, gate)
        .unwrap_or_else(|| ProbeOutcome::Failed(format!("no {gate} result was reported")))
}
