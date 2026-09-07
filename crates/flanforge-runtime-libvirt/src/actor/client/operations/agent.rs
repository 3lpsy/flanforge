use std::time::Duration;

use flanforge_libvirt_wire::{
    AgentExecOutcome, AgentExecRequest, HelperReply, HelperRequest, OwnershipManifest,
};

use crate::RuntimeError;

use super::{
    super::{conversion::unexpected, model::LibvirtActor},
    inner_timeout_seconds,
};

const AGENT_REPLY_SLACK: Duration = Duration::from_secs(1);

/// The helper clamps its own agent call into this range, so asking for more
/// would only mislead the caller about when it gives up.
const MAX_AGENT_TIMEOUT_SECONDS: u64 = 5;

/// Agent commands take the read-only lane. Each request is its own helper
/// process and a guest-side exec mutates no host resource, so a job that may
/// run for hours must not queue its status probes behind a cleanup.
impl LibvirtActor {
    pub(crate) async fn agent_probe(
        &self,
        manifest: OwnershipManifest,
        timeout: Duration,
    ) -> Result<bool, RuntimeError> {
        match self
            .request_read_only_with(timeout, |remaining| {
                Ok(HelperRequest::AgentProbe {
                    config: self.config.clone(),
                    manifest,
                    timeout_seconds: agent_timeout_seconds(remaining, "guest-agent probe")?,
                })
            })
            .await?
        {
            HelperReply::AgentProbe(is_exec_enabled) => Ok(is_exec_enabled),
            reply => unexpected(&reply),
        }
    }

    pub(crate) async fn agent_exec(
        &self,
        manifest: OwnershipManifest,
        request: AgentExecRequest,
        timeout: Duration,
    ) -> Result<i64, RuntimeError> {
        match self
            .request_read_only_with(timeout, |remaining| {
                Ok(HelperRequest::AgentExec {
                    config: self.config.clone(),
                    manifest,
                    request,
                    timeout_seconds: agent_timeout_seconds(remaining, "guest-agent exec")?,
                })
            })
            .await?
        {
            HelperReply::AgentStarted(pid) => Ok(pid),
            reply => unexpected(&reply),
        }
    }

    pub(crate) async fn agent_exec_status(
        &self,
        manifest: OwnershipManifest,
        pid: i64,
        timeout: Duration,
    ) -> Result<AgentExecOutcome, RuntimeError> {
        match self
            .request_read_only_with(timeout, |remaining| {
                Ok(HelperRequest::AgentExecStatus {
                    config: self.config.clone(),
                    manifest,
                    pid,
                    timeout_seconds: agent_timeout_seconds(remaining, "guest-agent exec status")?,
                })
            })
            .await?
        {
            HelperReply::AgentOutcome(outcome) => Ok(outcome),
            reply => unexpected(&reply),
        }
    }
}

fn agent_timeout_seconds(remaining: Duration, operation: &'static str) -> Result<u8, RuntimeError> {
    let seconds = inner_timeout_seconds(
        remaining,
        AGENT_REPLY_SLACK,
        MAX_AGENT_TIMEOUT_SECONDS,
        operation,
    )?;
    u8::try_from(seconds).map_err(RuntimeError::manifest)
}
