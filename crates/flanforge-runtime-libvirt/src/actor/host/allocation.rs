use std::time::{Duration, Instant};

use flanforge_libvirt_wire::{HelperReply, HelperRequest};
use virt::connect::Connect;

use crate::{RuntimeError, domain::guest_address};

use super::super::{
    guest,
    message::{CreateRequest, DefineRequest, ImportRequest},
    storage,
};

/// Everything one allocation's own volumes and domain need.
pub(super) fn allocation(
    connection: &Connect,
    request: HelperRequest,
) -> Result<HelperReply, RuntimeError> {
    match request {
        HelperRequest::CheckSource { config, pointer } => {
            storage::check_source_pointer(connection, &config, &pointer).map(|_| HelperReply::Unit)
        }
        HelperRequest::Create {
            config,
            manifest,
            source,
            seed,
            storage_bytes,
        } => Ok(HelperReply::Manifest(storage::create_guest_resources(
            connection,
            &config,
            CreateRequest {
                manifest,
                source,
                seed,
                storage_bytes,
            },
        )?)),
        HelperRequest::Define {
            config,
            manifest,
            cpu_count,
            memory_mb,
        } => {
            guest::define(
                connection,
                &config,
                &DefineRequest {
                    manifest,
                    cpu_count,
                    memory_mb,
                },
            )?;
            Ok(HelperReply::Unit)
        }
        HelperRequest::Start {
            config: _,
            manifest,
        } => {
            guest::start(connection, &manifest)?;
            Ok(HelperReply::Unit)
        }
        HelperRequest::Address {
            config: _,
            manifest,
            timeout_seconds,
        } => {
            let response = guest::agent_interfaces(
                connection,
                &manifest,
                Instant::now() + Duration::from_secs(u64::from(timeout_seconds)),
            )?;
            Ok(HelperReply::Address(guest_address(
                &response,
                manifest.mac_address(),
            )?))
        }
        HelperRequest::Cleanup {
            config,
            manifest,
            timeout_seconds,
        } => {
            guest::cleanup(
                connection,
                &config,
                &manifest,
                Instant::now() + Duration::from_secs(u64::from(timeout_seconds)),
            )?;
            Ok(HelperReply::Unit)
        }
        HelperRequest::Import {
            config,
            logical_name,
            volume_name,
            staged_image_path,
            manifest,
        } => Ok(HelperReply::Published(storage::import(
            connection,
            &config,
            ImportRequest {
                logical_name,
                volume_name,
                staged_image_path,
                manifest,
            },
        )?)),
        HelperRequest::DeleteVolume { config, key, name } => {
            storage::delete_exact(connection, &config.pool, key.as_deref(), &name)?;
            Ok(HelperReply::Unit)
        }
        request => agent(connection, request),
    }
}

/// Everything carried over one booted guest's QEMU guest agent. Split out
/// because the agent is a different authority from the host's storage: it
/// reaches inside a guest the daemon may have no route to.
fn agent(connection: &Connect, request: HelperRequest) -> Result<HelperReply, RuntimeError> {
    match request {
        HelperRequest::AgentProbe {
            config: _,
            manifest,
            timeout_seconds,
        } => {
            let deadline = deadline(timeout_seconds);
            guest::ping(connection, &manifest, deadline)?;
            Ok(HelperReply::AgentProbe(guest::is_exec_enabled(
                connection, &manifest, deadline,
            )?))
        }
        HelperRequest::AgentExec {
            config: _,
            manifest,
            request,
            timeout_seconds,
        } => Ok(HelperReply::AgentStarted(guest::exec(
            connection,
            &manifest,
            &request,
            deadline(timeout_seconds),
        )?)),
        HelperRequest::AgentExecStatus {
            config: _,
            manifest,
            pid,
            timeout_seconds,
        } => Ok(HelperReply::AgentOutcome(guest::exec_status(
            connection,
            &manifest,
            pid,
            deadline(timeout_seconds),
        )?)),
        request => super::warm(connection, request),
    }
}

fn deadline(timeout_seconds: u8) -> Instant {
    Instant::now() + Duration::from_secs(u64::from(timeout_seconds))
}
