use std::path::Path;

use anyhow::{Context, Result};
use flanforge_config::ConfigOverrides;
use flanforge_manager::HotGuestStatus;
use reqwest::Method;
use serde::Serialize;

use flanforge_cli::{HotCommand, HotListArgs};

use super::client::{NO_BODY, OperatorClient};

/// Whether a retirement waits for the current claim. Drain is the polite one.
#[derive(Debug, Serialize)]
pub(crate) struct HotRetireBody {
    pub(crate) evict: bool,
}

/// Lists, drains, or evicts the machines the hot pool holds.
///
/// # Errors
///
/// Returns an error when the configuration cannot be read, the service is
/// unreachable, or it rejects the request.
pub async fn run_hot_command(
    path: &Path,
    command: HotCommand,
    overrides: &ConfigOverrides,
) -> Result<()> {
    let client = OperatorClient::open(path, overrides).await?;
    match command {
        HotCommand::List(arguments) => list(&client, arguments).await,
        HotCommand::Drain(arguments) => retire(&client, arguments.vm_name.as_str(), false).await,
        HotCommand::Evict(arguments) => retire(&client, arguments.vm_name.as_str(), true).await,
    }
}

async fn list(client: &OperatorClient, arguments: HotListArgs) -> Result<()> {
    let guests: Vec<HotGuestStatus> = client
        .send(Method::GET, "/v1/operator/hot", NO_BODY)
        .await?;
    render(&guests, arguments.json)
}

async fn retire(client: &OperatorClient, vm_name: &str, is_evict: bool) -> Result<()> {
    let guests: Vec<HotGuestStatus> = client
        .send(
            Method::POST,
            &format!("/v1/operator/hot/{vm_name}"),
            Some(HotRetireBody { evict: is_evict }),
        )
        .await?;
    println!(
        "{vm_name} was {}",
        if is_evict { "evicted" } else { "drained" }
    );
    render(&guests, false)
}

fn render(guests: &[HotGuestStatus], is_json: bool) -> Result<()> {
    if is_json {
        println!(
            "{}",
            serde_json::to_string_pretty(guests).context("cannot render the hot guest listing")?
        );
        return Ok(());
    }
    if guests.is_empty() {
        println!("no hot guests");
        return Ok(());
    }
    println!(
        "{:<30}  {:<12}  {:<12}  {:<13}  {:>5}  {:>6}  {:>6}",
        "VM", "PROFILE", "LANE", "STATE", "JOBS", "AGE", "IDLE"
    );
    for guest in guests {
        println!(
            "{:<30}  {:<12}  {:<12}  {:<13}  {:>5}  {:>5}s  {:>5}s",
            guest.vm_name,
            guest.profile,
            format!("{:?}", guest.lane),
            state_label(guest),
            guest.jobs_served,
            guest.age_seconds,
            guest.idle_seconds
        );
    }
    Ok(())
}

/// A record whose machine the host does not report is named as such: the
/// difference between "idle" and "idle, and gone" is the whole diagnosis.
fn state_label(guest: &HotGuestStatus) -> String {
    if guest.is_machine_present {
        format!("{:?}", guest.state)
    } else {
        format!("{:?}!", guest.state)
    }
}
