use std::path::Path;

use anyhow::Result;
use flanforge_manager::SweepReport;
use reqwest::Method;
use serde::Serialize;

use crate::cli::ReaperCommand;

use super::client::OperatorClient;

/// What the sweep is allowed to do. Absent or false plans only.
#[derive(Debug, Serialize)]
pub(crate) struct ReapBody {
    pub(crate) delete: bool,
}

/// Runs one sweep against the running service.
///
/// # Errors
///
/// Returns an error when the configuration cannot be read, the service is
/// unreachable, or the sweep fails.
pub async fn run_reaper_command(path: &Path, command: ReaperCommand) -> Result<()> {
    let ReaperCommand::Run(arguments) = command;
    let client = OperatorClient::open(path).await?;
    let report: SweepReport = client
        .send(
            Method::POST,
            "/v1/operator/reap",
            Some(ReapBody {
                delete: arguments.delete,
            }),
        )
        .await?;
    print_report(&report, arguments.delete);
    Ok(())
}

fn print_report(report: &SweepReport, is_delete: bool) {
    if !is_delete {
        println!("dry run: nothing was deleted");
    }
    if let Some(reason) = report.inert_reason {
        println!("inert: {reason:?}; nothing on this host can be aged");
    }
    println!("planned: {}", report.planned.len());
    for candidate in &report.planned {
        println!(
            "  {} ({:?}, {:?}, {}s)",
            candidate.name, candidate.state, candidate.authorization, candidate.age_seconds
        );
    }
    if !report.deleted.is_empty() {
        println!("deleted: {}", report.deleted.join(", "));
    }
    if !report.skipped.is_empty() {
        println!("skipped: {}", report.skipped.join(", "));
    }
    if !report.pruned_records.is_empty() {
        println!(
            "records dropped: {}",
            report
                .pruned_records
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::cli::{Arguments, ReaperCommand, TopCommand};

    use super::run_reaper_command;

    /// Built through clap, so the test covers the flag as an operator types it.
    fn command(delete: bool) -> ReaperCommand {
        let mut arguments = vec!["flanforged", "reaper", "run"];
        if delete {
            arguments.push("--delete");
        }
        let parsed = Arguments::try_parse_from(arguments)
            .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Reaper(reaper) = parsed.command else {
            unreachable!("reaper command")
        };
        reaper.command
    }

    /// Serves one canned sweep report and returns the request it received.
    async fn capture_one_request(listener: tokio::net::TcpListener) -> String {
        let (mut stream, _) = listener
            .accept()
            .await
            .unwrap_or_else(|error| unreachable!("accept: {error}"));
        let mut request = vec![0_u8; 4_096];
        let read = stream
            .read(&mut request)
            .await
            .unwrap_or_else(|error| unreachable!("read: {error}"));
        let body = r#"{"planned":[],"deleted":[],"skipped":[],"finished_at_unix":0}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .unwrap_or_else(|error| unreachable!("write: {error}"));
        let _ = stream.shutdown().await;
        String::from_utf8_lossy(&request[..read]).into_owned()
    }

    async fn run_against_a_fake_service(delete: bool) -> String {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let path = directory.path().join("config.toml");
        let state_dir = directory.path().join("state");
        std::fs::create_dir_all(&state_dir)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        std::fs::write(
            &path,
            flanforge_config::STARTER_CONFIG
                .replace(
                    r#"listen = "127.0.0.1:9843""#,
                    &format!(r#"listen = "{address}""#),
                )
                .replace(
                    r#"state_dir = "~/Library/Application Support/flanforge/state""#,
                    &format!(r#"state_dir = "{}""#, state_dir.display()),
                ),
        )
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        // The daemon writes this at startup; the CLI presents it on every call.
        flanforge_manager::ensure_operator_token(&state_dir)
            .await
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));

        let served = tokio::spawn(capture_one_request(listener));
        run_reaper_command(&path, command(delete))
            .await
            .unwrap_or_else(|error| unreachable!("reaper run: {error}"));
        served
            .await
            .unwrap_or_else(|error| unreachable!("join: {error}"))
    }

    #[tokio::test]
    async fn reaper_run_requires_delete_to_delete() {
        let planned = run_against_a_fake_service(false).await;
        assert!(planned.starts_with("POST /v1/operator/reap "), "{planned}");
        assert!(planned.ends_with(r#"{"delete":false}"#), "{planned}");

        let deleting = run_against_a_fake_service(true).await;
        assert!(deleting.ends_with(r#"{"delete":true}"#), "{deleting}");
    }
}
