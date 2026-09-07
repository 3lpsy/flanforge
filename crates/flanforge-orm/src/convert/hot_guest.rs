use flanforge_core::{CloneSource, GuestSize, HotGuest};
use validator::Validate;

use super::{
    ConvertError,
    token::{
        from_token, optional_from_token, optional_to_i64, optional_to_u64, optional_token, to_i64,
        to_token, to_u64,
    },
};
use crate::entities::hot_guest::Model;

/// # Errors
///
/// Returns an error when a value does not fit its column.
pub fn hot_guest_to_model(guest: &HotGuest) -> Result<Model, ConvertError> {
    Ok(Model {
        vm_name: to_token("vm_name", &guest.vm_name)?,
        profile: to_token("profile", &guest.profile)?,
        lane: to_token("lane", &guest.lane)?,
        state: to_token("state", &guest.state)?,
        size_cpu_count: i64::from(guest.size.cpu_count),
        size_memory_mb: i64::from(guest.size.memory_mb),
        size_storage_mb: to_i64("size_storage_mb", guest.size.storage_mb)?,
        source_name: to_token("source_name", &guest.source.name)?,
        source_kind: to_token("source_kind", &guest.source.kind)?,
        source_fingerprint: optional_token(
            "source_fingerprint",
            guest.source.base_fingerprint.as_ref(),
        )?,
        source_fallback_reason: optional_token(
            "source_fallback_reason",
            guest.source.fallback_reason.as_ref(),
        )?,
        warm_generation: optional_to_i64("warm_generation", guest.warm_generation)?,
        age_limit_seconds: optional_to_i64("age_limit_seconds", guest.age_limit_seconds)?,
        booted_at_unix: to_i64("booted_at_unix", guest.booted_at_unix)?,
        updated_at_unix: to_i64("updated_at_unix", guest.updated_at_unix)?,
        jobs_served: i64::from(guest.jobs_served),
        claimed_by: optional_token("claimed_by", guest.claimed_by.as_ref())?,
        drain_reason: optional_token("drain_reason", guest.drain_reason.as_ref())?,
    })
}

/// Rebuilds the record and re-runs its validator, including the cross-field
/// claim invariant only a `Claimed` row may carry `claimed_by`.
///
/// # Errors
///
/// Returns an error when the row cannot be mapped or does not validate.
pub fn hot_guest_from_model(model: &Model) -> Result<HotGuest, ConvertError> {
    let guest = HotGuest {
        vm_name: from_token("vm_name", &model.vm_name)?,
        profile: from_token("profile", &model.profile)?,
        lane: from_token("lane", &model.lane)?,
        state: from_token("state", &model.state)?,
        size: GuestSize {
            cpu_count: u8::try_from(model.size_cpu_count).map_err(|_| ConvertError::Numeric {
                field: "size_cpu_count",
            })?,
            memory_mb: u32::try_from(model.size_memory_mb).map_err(|_| ConvertError::Numeric {
                field: "size_memory_mb",
            })?,
            storage_mb: to_u64("size_storage_mb", model.size_storage_mb)?,
        },
        source: CloneSource {
            name: from_token("source_name", &model.source_name)?,
            kind: from_token("source_kind", &model.source_kind)?,
            base_fingerprint: optional_from_token(
                "source_fingerprint",
                model.source_fingerprint.as_deref(),
            )?,
            fallback_reason: optional_from_token(
                "source_fallback_reason",
                model.source_fallback_reason.as_deref(),
            )?,
        },
        warm_generation: optional_to_u64("warm_generation", model.warm_generation)?,
        age_limit_seconds: optional_to_u64("age_limit_seconds", model.age_limit_seconds)?,
        booted_at_unix: to_u64("booted_at_unix", model.booted_at_unix)?,
        updated_at_unix: to_u64("updated_at_unix", model.updated_at_unix)?,
        jobs_served: u32::try_from(model.jobs_served).map_err(|_| ConvertError::Numeric {
            field: "jobs_served",
        })?,
        claimed_by: optional_from_token("claimed_by", model.claimed_by.as_deref())?,
        drain_reason: optional_from_token("drain_reason", model.drain_reason.as_deref())?,
    };
    if guest.validate().is_err() {
        return Err(ConvertError::Invalid {
            id: model.vm_name.clone(),
        });
    }
    Ok(guest)
}
