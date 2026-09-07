use crate::{
    WireError,
    libvirt_helper::{
        MAX_LIBVIRT_HELPER_FAILURE_BYTES,
        limits::{MAX_SAFE_NAME_BYTES, MAX_VOLUME_KEY_BYTES},
        model::HelperReply,
    },
    validation::{is_safe_key, is_safe_name},
};

use super::{
    agent::ensure_outcome_valid,
    shared::{MAX_RETIREMENT_POINTERS, invalid},
};

pub(in crate::libvirt_helper) fn reply(reply: &HelperReply) -> Result<(), WireError> {
    match reply {
        HelperReply::Unit | HelperReply::Address(_) | HelperReply::AgentProbe(_) => Ok(()),
        HelperReply::AgentStarted(pid) => {
            if *pid <= 0 {
                return invalid("agent started");
            }
            Ok(())
        }
        HelperReply::AgentOutcome(outcome) => ensure_outcome_valid(outcome),
        HelperReply::Manifest(manifest) => manifest.ensure_valid(),
        HelperReply::Published(publication) => publication.ensure_valid(),
        HelperReply::Warm(pointer) => pointer.ensure_valid(),
        HelperReply::Retired(keys) => {
            if keys.len() > MAX_RETIREMENT_POINTERS
                || keys
                    .iter()
                    .any(|key| !is_safe_key(key, MAX_VOLUME_KEY_BYTES))
            {
                return invalid("retired");
            }
            Ok(())
        }
        HelperReply::Inventory(machines) => {
            if machines.len() > 4_096
                || machines.iter().any(|machine| {
                    !is_safe_name(&machine.name, MAX_SAFE_NAME_BYTES)
                        || machine.size.is_some_and(|size| {
                            size.cpu_count == 0 || size.memory_mb == 0 || size.storage_mb == 0
                        })
                })
            {
                return invalid("inventory");
            }
            Ok(())
        }
        HelperReply::Error(failure) => {
            if failure.message().is_empty()
                || failure.message().len() > MAX_LIBVIRT_HELPER_FAILURE_BYTES
                || failure
                    .message()
                    .bytes()
                    .any(|byte| byte.is_ascii_control())
            {
                return invalid("error");
            }
            Ok(())
        }
    }
}
