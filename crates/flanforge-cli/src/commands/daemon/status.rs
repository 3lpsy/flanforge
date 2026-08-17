use std::path::Path;

use anyhow::Result;
use flanforge_manager::OperatorStatus;
use reqwest::Method;

use super::{
    super::client::{NO_BODY, OperatorClient},
    control::{last_exit_code, launchctl_output, launchd_identity},
    paths::{SERVICE_LABEL, ServicePaths},
};

/// Reports what launchd knows about the service, then what the service itself
/// reports. Changes nothing.
pub(super) async fn status(config_path: &Path) -> Result<()> {
    let paths = ServicePaths::discover()?;
    println!("label:      {SERVICE_LABEL}");
    println!("binary:     {}", describe(&paths.binary).await);
    println!("agent:      {}", describe(&paths.launch_agent).await);
    if !paths.launch_agent.is_file() {
        println!("state:      not installed");
        return Ok(());
    }
    let (_, target) = launchd_identity().await?;
    let Ok(report) = launchctl_output(["print".into(), target]).await else {
        println!("state:      not loaded");
        return Ok(());
    };
    let state = report_field(&report, "state = ").unwrap_or("unknown");
    println!("state:      {state}");
    if let Some(pid) = report_field(&report, "pid = ") {
        println!("pid:        {pid}");
    }
    if let Some(code) = last_exit_code(&report) {
        println!("last exit:  {code}");
    }
    print_service_status(config_path).await;
    Ok(())
}

/// The service's own view, best effort: a daemon that is not answering is
/// already reported above.
async fn print_service_status(config_path: &Path) {
    let Ok(client) = OperatorClient::open(config_path).await else {
        return;
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
        "allocations:{} active, {} foreign VM(s) running, limit {}",
        capacity.active_allocations, capacity.foreign_running, capacity.max_running_vms
    );
    println!(
        "committed:  {} vCPU, {} MB{}",
        capacity.committed_cpu_count,
        capacity.committed_memory_mb,
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
}

fn budget(capacity: &flanforge_manager::CapacityStatus) -> String {
    match (capacity.host_cpu_count, capacity.host_memory_mb) {
        (Some(cpu_count), Some(memory_mb)) => format!(" of {cpu_count} vCPU, {memory_mb} MB"),
        _ => " of an unset budget (allocation is serialized)".to_owned(),
    }
}

async fn describe(path: &Path) -> String {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => format!("{} ({} bytes)", path.display(), metadata.len()),
        Err(_) => format!("{} (absent)", path.display()),
    }
}

fn report_field<'a>(report: &'a str, key: &str) -> Option<&'a str> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(key))
        .map(str::trim)
}
