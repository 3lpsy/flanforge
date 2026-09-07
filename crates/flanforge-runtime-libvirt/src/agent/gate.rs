use std::time::Duration;

use flanforge_manager::WorkerError;
use flanforge_runtime::{
    GuestCapture, GuestChannel, GuestCommand, GuestExit, GuestProgram, GuestSession,
};

use super::ready::{GuestReadiness, MAX_REPORT_BYTES};

pub(crate) const GUEST_READY_HELPER: &str = "/usr/local/libexec/flanforge-guest-ready";

/// The image contract this daemon speaks. The helper reports its own, and a
/// mismatch is a refusal rather than a best-effort read of unknown fields.
pub(crate) const GUEST_CONTRACT_VERSION: u8 = 2;

/// Exit codes the baked helper promises. Anything else is read as terminal:
/// an unrecognised code is not evidence that waiting would help.
const READY: i32 = 0;
const RETRY: i32 = 10;

/// The helper's own `--wait-seconds` bound, and the slack left so its reply
/// still fits inside the caller's deadline.
const MAX_WAIT_SECONDS: u64 = 120;
const REPLY_SLACK_SECONDS: u64 = 10;

/// Waits until the guest reports first-boot provisioning complete, or names the
/// gate that failed.
///
/// Agent liveness does not imply provisioning finished, so this is a real gate
/// rather than a reachability check — and it runs over whichever channel is
/// bound, so SSH gets the same diagnostics rather than a boot timeout.
pub(crate) async fn ensure_provisioned(
    channel: &dyn GuestChannel,
    session: &GuestSession,
    runner_user: &str,
    contract_version: u8,
    deadline: tokio::time::Instant,
    poll: Duration,
) -> Result<(), WorkerError> {
    loop {
        let arguments = [
            "--wait-seconds".to_owned(),
            wait_seconds(deadline).to_string(),
        ];
        let command = GuestCommand::new(
            GuestProgram::Program {
                path: GUEST_READY_HELPER,
                arguments: &arguments,
            },
            None,
            GuestCapture::Bounded(MAX_REPORT_BYTES),
        )?;
        let output = channel.run(session, &command).await?;
        let GuestExit::Code(code) = output.exit() else {
            return Err(WorkerError::new(
                "the guest readiness probe was terminated by a signal",
            ));
        };
        if output.is_truncated() {
            return Err(WorkerError::new("guest readiness report is oversized"));
        }
        // An empty or malformed reply leaves two clues: the helper's exit code
        // (64 usage, 11 with only stderr, 127 exec failure) and its stderr.
        let report = GuestReadiness::parse(output.stdout(), contract_version).map_err(|error| {
            let stderr = printable_prefix(output.stderr());
            if stderr.is_empty() {
                WorkerError::new(format!("{error} (helper exit {code})"))
            } else {
                WorkerError::new(format!("{error} (helper exit {code}: {stderr})"))
            }
        })?;
        match code {
            READY if report.is_ready() => {
                ensure_account_matches(&report, runner_user)?;
                tracing::info!(gate = %report.gate(), "guest provisioning is complete");
                return Ok(());
            }
            RETRY => {
                tracing::debug!(cause = report.cause(), "waiting for guest provisioning");
            }
            // A named cause now beats the same failure discovered at the boot
            // timeout with nothing to show for the wait.
            _ => return Err(WorkerError::new(report.cause())),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(WorkerError::new(format!(
                "guest provisioning did not finish in time: {}",
                report.cause()
            )));
        }
        tokio::time::sleep(poll).await;
    }
}

/// The account is baked into the image, so a configuration naming a different
/// one would silently run every job somewhere the image never prepared.
fn ensure_account_matches(report: &GuestReadiness, runner_user: &str) -> Result<(), WorkerError> {
    let (name, uid) = report.account();
    if name == runner_user {
        return Ok(());
    }
    Err(WorkerError::new(format!(
        "the base image runs jobs as {name} (uid {uid}), not the configured {runner_user}"
    )))
}

/// The guest writes this stream, so only a short printable prefix may enter an
/// error message.
fn printable_prefix(stream: &[u8]) -> String {
    stream
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                ' '
            }
        })
        .take(256)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Re-derived on every attempt so the last probe cannot outlive the deadline it
/// was meant to report before.
fn wait_seconds(deadline: tokio::time::Instant) -> u64 {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .map(|remaining| remaining.as_secs().saturating_sub(REPLY_SLACK_SECONDS))
        .unwrap_or_default()
        .clamp(1, MAX_WAIT_SECONDS)
}
