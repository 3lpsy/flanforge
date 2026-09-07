use std::time::{Duration, Instant};

use flanforge_libvirt_wire::{HelperReply, HelperRequest};
use virt::connect::Connect;

use crate::RuntimeError;

use super::super::{
    guest,
    message::{WarmCaptureRequest, WarmVerifyRequest},
    storage,
};

pub(super) fn warm(
    connection: &Connect,
    request: HelperRequest,
) -> Result<HelperReply, RuntimeError> {
    match request {
        HelperRequest::WarmQuiesce {
            config: _,
            manifest,
            timeout_seconds,
        } => {
            guest::quiesce(
                connection,
                &manifest,
                Instant::now() + Duration::from_secs(u64::from(timeout_seconds)),
            )?;
            Ok(HelperReply::Unit)
        }
        HelperRequest::WarmCapture {
            config,
            manifest,
            profile,
            capture_id,
            generation,
            virtual_bytes,
        } => Ok(HelperReply::Warm(storage::capture(
            connection,
            &config,
            &WarmCaptureRequest {
                manifest,
                profile,
                capture_id,
                generation,
                virtual_bytes,
            },
        )?)),
        HelperRequest::WarmVerify {
            config,
            profile,
            capture_id,
            generation,
            produced_at_unix,
            virtual_bytes,
        } => Ok(HelperReply::Warm(storage::verify(
            connection,
            &config,
            &WarmVerifyRequest {
                profile,
                capture_id,
                generation,
                produced_at_unix,
                virtual_bytes,
            },
        )?)),
        HelperRequest::WarmRetire {
            config,
            candidates,
            protected,
        } => Ok(HelperReply::Retired(storage::retire(
            connection,
            &config,
            &candidates,
            &protected,
        )?)),
        request => Err(RuntimeError::helper(format!(
            "libvirt helper cannot dispatch {}",
            super::operation(&request)
        ))),
    }
}
