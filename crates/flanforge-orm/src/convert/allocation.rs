use flanforge_core::{Allocation, AllocationRequest, CloneSource, GuestSize};
use validator::Validate;

use super::{
    ConvertError,
    token::{
        from_json, from_token, optional_from_token, optional_to_i64, optional_to_u64,
        optional_token, to_i64, to_json, to_token, to_u64,
    },
};
use crate::entities::allocation::Model;

/// Flattens an allocation onto its row. Filterable facts become columns;
/// `origin` and `retention` travel as whole JSON documents.
///
/// # Errors
///
/// Returns an error when a value does not fit its column.
pub fn allocation_to_model(allocation: &Allocation) -> Result<Model, ConvertError> {
    let size = allocation.size.as_ref();
    let source = allocation.source.as_ref();
    Ok(Model {
        id: allocation.id.to_string(),
        profile: to_token("profile", &allocation.request.profile)?,
        repository: to_token("repository", &allocation.request.repository)?,
        run_id: to_i64("run_id", allocation.request.run_id)?,
        run_attempt: i64::from(allocation.request.run_attempt),
        state: to_token("state", &allocation.state)?,
        mode: to_token("mode", &allocation.mode)?,
        origin: to_json("origin", &allocation.origin)?,
        vm_name: to_token("vm_name", &allocation.vm_name)?,
        runner_label: to_token("runner_label", &allocation.runner_label)?,
        vm_created: allocation.vm_created,
        runner_id: allocation.runner_id,
        created_at_unix: to_i64("created_at_unix", allocation.created_at_unix)?,
        updated_at_unix: to_i64("updated_at_unix", allocation.updated_at_unix)?,
        error: allocation.error.clone(),
        size_cpu_count: size.map(|size| i64::from(size.cpu_count)),
        size_memory_mb: size.map(|size| i64::from(size.memory_mb)),
        size_storage_mb: size
            .map(|size| to_i64("size_storage_mb", size.storage_mb))
            .transpose()?,
        source_name: optional_token("source_name", source.map(|source| &source.name))?,
        source_kind: optional_token("source_kind", source.map(|source| &source.kind))?,
        source_fingerprint: optional_token(
            "source_fingerprint",
            source.and_then(|source| source.base_fingerprint.as_ref()),
        )?,
        source_fallback_reason: optional_token(
            "source_fallback_reason",
            source.and_then(|source| source.fallback_reason.as_ref()),
        )?,
        hot_lane: optional_token("hot_lane", allocation.hot_lane.as_ref())?,
        hot_refusal: optional_token("hot_refusal", allocation.hot_refusal.as_ref())?,
        hot_age_seconds: optional_to_i64("hot_age_seconds", allocation.hot_age_seconds)?,
        warm_generation: optional_to_i64("warm_generation", allocation.warm_generation)?,
        retention: allocation
            .retention
            .as_ref()
            .map(|retention| to_json("retention", retention))
            .transpose()?,
        terminal_reason: optional_token("terminal_reason", allocation.terminal_reason.as_ref())?,
    })
}

/// Rebuilds the domain record and re-runs its validator, exactly as the JSON
/// store did on read: a row that cannot be trusted is an error, not a record.
///
/// # Errors
///
/// Returns an error when the row cannot be mapped or does not validate.
pub fn allocation_from_model(model: &Model) -> Result<Allocation, ConvertError> {
    let allocation = Allocation {
        id: from_token("id", &model.id)?,
        request: AllocationRequest {
            profile: from_token("profile", &model.profile)?,
            repository: from_token("repository", &model.repository)?,
            run_id: to_u64("run_id", model.run_id)?,
            run_attempt: u32::try_from(model.run_attempt).map_err(|_| ConvertError::Numeric {
                field: "run_attempt",
            })?,
        },
        state: from_token("state", &model.state)?,
        vm_name: from_token("vm_name", &model.vm_name)?,
        runner_label: from_token("runner_label", &model.runner_label)?,
        vm_created: model.vm_created,
        runner_id: model.runner_id,
        created_at_unix: to_u64("created_at_unix", model.created_at_unix)?,
        updated_at_unix: to_u64("updated_at_unix", model.updated_at_unix)?,
        error: model.error.clone(),
        mode: from_token("mode", &model.mode)?,
        size: size_from_model(model)?,
        source: source_from_model(model)?,
        origin: from_json("origin", &model.origin)?,
        hot_lane: optional_from_token("hot_lane", model.hot_lane.as_deref())?,
        hot_refusal: optional_from_token("hot_refusal", model.hot_refusal.as_deref())?,
        hot_age_seconds: optional_to_u64("hot_age_seconds", model.hot_age_seconds)?,
        warm_generation: optional_to_u64("warm_generation", model.warm_generation)?,
        retention: model
            .retention
            .as_deref()
            .map(|retention| from_json("retention", retention))
            .transpose()?,
        terminal_reason: optional_from_token("terminal_reason", model.terminal_reason.as_deref())?,
    };
    if allocation.validate().is_err() {
        return Err(ConvertError::Invalid {
            id: model.id.clone(),
        });
    }
    Ok(allocation)
}

fn size_from_model(model: &Model) -> Result<Option<GuestSize>, ConvertError> {
    match (
        model.size_cpu_count,
        model.size_memory_mb,
        model.size_storage_mb,
    ) {
        (None, None, None) => Ok(None),
        (Some(cpu_count), Some(memory_mb), Some(storage_mb)) => Ok(Some(GuestSize {
            cpu_count: u8::try_from(cpu_count).map_err(|_| ConvertError::Numeric {
                field: "size_cpu_count",
            })?,
            memory_mb: u32::try_from(memory_mb).map_err(|_| ConvertError::Numeric {
                field: "size_memory_mb",
            })?,
            storage_mb: to_u64("size_storage_mb", storage_mb)?,
        })),
        _ => Err(ConvertError::SplitGroup { field: "size" }),
    }
}

fn source_from_model(model: &Model) -> Result<Option<CloneSource>, ConvertError> {
    match (model.source_name.as_deref(), model.source_kind.as_deref()) {
        (None, None) => Ok(None),
        (Some(name), Some(kind)) => Ok(Some(CloneSource {
            name: from_token("source_name", name)?,
            kind: from_token("source_kind", kind)?,
            base_fingerprint: optional_from_token(
                "source_fingerprint",
                model.source_fingerprint.as_deref(),
            )?,
            fallback_reason: optional_from_token(
                "source_fallback_reason",
                model.source_fallback_reason.as_deref(),
            )?,
        })),
        _ => Err(ConvertError::SplitGroup { field: "source" }),
    }
}
