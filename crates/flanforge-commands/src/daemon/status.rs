use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;
use flanforge_config::ConfigOverrides;
use flanforge_manager::OperatorStatus;
use reqwest::Method;

use super::{
    super::client::{NO_BODY, OperatorClient},
    service,
};

/// Reports what the native service manager knows, then what the service itself
/// reports. Changes nothing.
pub(super) async fn status(config_path: &Path, overrides: &ConfigOverrides) -> Result<()> {
    let service = service::native().status().await?;
    service.print();
    if !service.is_installed {
        return Ok(());
    }
    print_service_status(config_path, overrides).await;
    Ok(())
}

/// `launchctl` returns before the service binds its listener, so a report taken
/// straight after a start would say "not answering" for a healthy daemon. Polls
/// briefly and returns either way; the report itself is what tells the operator.
pub(super) async fn wait_until_answering(
    config_path: &Path,
    bound: Duration,
    overrides: &ConfigOverrides,
) {
    let deadline = Instant::now() + bound;
    while !is_answering(config_path, overrides).await {
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn is_answering(config_path: &Path, overrides: &ConfigOverrides) -> bool {
    let Ok(client) = OperatorClient::open(config_path, overrides).await else {
        return false;
    };
    let answer: Result<OperatorStatus> = client
        .send(Method::GET, "/v1/operator/status", NO_BODY)
        .await;
    answer.is_ok()
}

/// The service's own view, best effort: a daemon that is not answering is
/// already reported above.
pub(super) async fn print_service_status(config_path: &Path, overrides: &ConfigOverrides) {
    let client = match OperatorClient::open(config_path, overrides).await {
        Ok(client) => client,
        Err(error) => {
            // Returning in silence truncated the report into one that read as
            // a healthy service with nothing to add.
            let cause = format!("{error:#}");
            tracing::warn!(%cause, "the operator credential did not open, so the report has no service section");
            println!("{}", unavailable_line(&cause));
            return;
        }
    };
    let Ok(status): Result<OperatorStatus> = client
        .send(Method::GET, "/v1/operator/status", NO_BODY)
        .await
    else {
        println!("service:    not answering");
        return;
    };
    let capacity = status.capacity;
    println!(
        "runtime:    {} ({:?})",
        status.runtime.backend(),
        status.runtime.health()
    );
    println!(
        "allocations:{} active, {} foreign VM(s) running, limit {}",
        capacity.active_allocations, capacity.foreign_running, capacity.max_running_vms
    );
    // Only when the pool is configured: a host that never enables hot keeps
    // its status output byte-identical.
    if capacity.max_hot_vms > 0 || capacity.hot_running > 0 {
        println!(
            "hot pool:   {} machine(s) holding a slot of {} allowed, against a limit of {}",
            capacity.hot_running, capacity.max_hot_vms, capacity.max_running_vms
        );
    }
    println!(
        "committed:  {} vCPU, {} MB memory, {} MB storage{}",
        capacity.committed_cpu_count,
        capacity.committed_memory_mb,
        capacity.committed_storage_mb,
        budget(&capacity)
    );
    if !capacity.is_host_visible {
        println!("host:       not listable; admission answers busy");
    }
    // These are process-local: blank means "not since this daemon started".
    println!(
        "config:     generation {} since start",
        status.config_generation
    );
    if !status.restart_pending.is_empty() {
        println!("restart:    {}", status.restart_pending.join(", "));
    }
    for image in &status.warm_images {
        println!(
            "warm:       {} -> {} (generation {}, {:?}{}{})",
            image.profile,
            image.warm_template,
            image.generation,
            image.state,
            if image.is_referenced {
                ""
            } else {
                ", unreferenced"
            },
            if image.is_quarantined {
                ", quarantined"
            } else {
                ""
            }
        );
        // Only the pinned case prints, so steady state stays byte-identical.
        if let Some(retained) = image.retained_generations.filter(|count| *count > 0) {
            println!(
                "warm retained: {} ({retained} generation(s) pinned by live or recoverable overlays)",
                image.profile
            );
        }
    }
    // Only deleting sweeps are recorded, so a dry run cannot masquerade as one.
    match &status.last_sweep {
        Some(sweep) => println!(
            "last sweep: {} planned, {} deleted, {} record(s) dropped (this process){}",
            sweep.planned.len(),
            sweep.deleted.len(),
            sweep.pruned_records.len(),
            match sweep.inert_reason {
                Some(reason) => format!(" [inert: {reason:?}]"),
                None => String::new(),
            }
        ),
        None => println!("last sweep: none in this process"),
    }
    // Only the populated case prints, so steady state stays byte-identical.
    if let Some(unaged) = status
        .last_sweep
        .as_ref()
        .map(|sweep| &sweep.unaged)
        .filter(|unaged| !unaged.is_empty())
    {
        println!(
            "never collectable (no determinable age): {}",
            unaged.join(", ")
        );
    }
}

/// The report line for a credential that will not open, held apart from the
/// `not answering` line a reachable daemon produces. The cause is the whole
/// error chain, which the credential reader keeps the token out of.
pub(super) fn unavailable_line(cause: &str) -> String {
    format!("service:    unavailable; {cause}")
}

fn budget(capacity: &flanforge_manager::CapacityStatus) -> String {
    match (
        capacity.host_cpu_count,
        capacity.host_memory_mb,
        capacity.host_storage_mb,
    ) {
        (Some(cpu_count), Some(memory_mb), Some(storage_mb)) => {
            format!(" of {cpu_count} vCPU, {memory_mb} MB memory, {storage_mb} MB storage")
        }
        (Some(cpu_count), Some(memory_mb), None) => {
            format!(" of {cpu_count} vCPU, {memory_mb} MB memory")
        }
        _ => " of an unset budget (allocation is serialized)".to_owned(),
    }
}
