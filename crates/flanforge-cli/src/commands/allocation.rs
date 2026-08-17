use std::path::Path;

use anyhow::{Context, Result};
use flanforge_core::Allocation;
use flanforge_manager::AllocationSummary;
use reqwest::Method;

use crate::cli::{AllocationCommand, AllocationListArgs};

use super::client::{NO_BODY, OperatorClient};

/// Lists or cancels allocations on the running service.
///
/// # Errors
///
/// Returns an error when the configuration cannot be read, the service is
/// unreachable, or it rejects the request.
pub async fn run_allocation_command(path: &Path, command: AllocationCommand) -> Result<()> {
    let client = OperatorClient::open(path).await?;
    match command {
        AllocationCommand::List(arguments) => list(&client, arguments).await,
        AllocationCommand::Cancel(arguments) => cancel(&client, &arguments.id.to_string()).await,
    }
}

async fn list(client: &OperatorClient, arguments: AllocationListArgs) -> Result<()> {
    let summaries: Vec<AllocationSummary> = client
        .send(Method::GET, "/v1/operator/allocations", NO_BODY)
        .await?;
    if arguments.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summaries)
                .context("cannot render the allocation listing")?
        );
        return Ok(());
    }
    if summaries.is_empty() {
        println!("no allocations");
        return Ok(());
    }
    println!(
        "{:<36}  {:<12}  {:<20}  {:<10}  {:<16}  {:>6}",
        "ID", "PROFILE", "REPOSITORY", "RUN", "STATE", "AGE"
    );
    for summary in summaries {
        println!(
            "{:<36}  {:<12}  {:<20}  {:<10}  {:<16}  {:>5}s",
            summary.id,
            summary.profile,
            summary.repository,
            format!("{}/{}", summary.run_id, summary.run_attempt),
            format!("{:?}", summary.state),
            summary.age_seconds
        );
    }
    Ok(())
}

async fn cancel(client: &OperatorClient, id: &str) -> Result<()> {
    let allocation: Allocation = client
        .send(
            Method::DELETE,
            &format!("/v1/operator/allocations/{id}"),
            NO_BODY,
        )
        .await?;
    println!("{} is {:?}", allocation.id, allocation.state);
    Ok(())
}
