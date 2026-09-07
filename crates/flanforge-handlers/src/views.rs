//! Projections from manager and entity types onto the wire views.

use flanforge_manager::{AllocationSummary, HotGuestStatus, SweepReport, WarmImageStatus};
use flanforge_orm::entities::{allocation, event};
use flanforge_wire::{
    AllocationRecordView, AllocationView, EventView, HotGuestView, SweepView, WarmImageView,
};
use serde::Serialize;

/// The serde token of a value whose serialized form is one string — the same
/// spelling the database columns and the workflow API use.
pub(crate) fn token<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(token)) => token,
        _ => "unknown".to_owned(),
    }
}

pub(crate) fn allocation_view(summary: &AllocationSummary) -> AllocationView {
    AllocationView {
        id: summary.id.to_string(),
        profile: summary.profile.as_str().to_owned(),
        repository: token(&summary.repository),
        run_id: summary.run_id,
        run_attempt: summary.run_attempt,
        state: token(&summary.state),
        mode: token(&summary.mode),
        vm_name: summary.vm_name.as_str().to_owned(),
        age_seconds: summary.age_seconds,
        cpu_count: summary.size.map(|size| size.cpu_count),
        memory_mb: summary.size.map(|size| size.memory_mb),
        storage_mb: summary.size.map(|size| size.storage_mb),
        source: summary
            .source
            .as_ref()
            .map(|source| source.name.as_str().to_owned()),
    }
}

pub(crate) fn allocation_record_view(model: &allocation::Model) -> AllocationRecordView {
    AllocationRecordView {
        id: model.id.clone(),
        profile: model.profile.clone(),
        repository: model.repository.clone(),
        run_id: model.run_id,
        run_attempt: model.run_attempt,
        state: model.state.clone(),
        mode: model.mode.clone(),
        vm_name: model.vm_name.clone(),
        created_at_unix: model.created_at_unix,
        updated_at_unix: model.updated_at_unix,
        error: model.error.clone(),
        terminal_reason: model.terminal_reason.clone(),
        warm_generation: model.warm_generation,
    }
}

pub(crate) fn hot_guest_view(status: &HotGuestStatus) -> HotGuestView {
    HotGuestView {
        vm_name: status.vm_name.as_str().to_owned(),
        profile: status.profile.as_str().to_owned(),
        lane: token(&status.lane),
        state: token(&status.state),
        age_seconds: status.age_seconds,
        idle_seconds: status.idle_seconds,
        jobs_served: status.jobs_served,
        claimed_by: status.claimed_by.map(|id| id.to_string()),
        drain_reason: status.drain_reason.as_ref().map(token),
        is_machine_present: status.is_machine_present,
    }
}

pub(crate) fn warm_image_view(status: &WarmImageStatus) -> WarmImageView {
    WarmImageView {
        profile: status.profile.as_str().to_owned(),
        warm_template: status.warm_template.as_str().to_owned(),
        generation: status.generation,
        state: token(&status.state),
        produced_at_unix: status.produced_at_unix,
        is_referenced: status.is_referenced,
        is_quarantined: status.is_quarantined,
        retained_generations: status.retained_generations,
    }
}

pub(crate) fn sweep_view(report: &SweepReport) -> SweepView {
    SweepView {
        planned: report.planned.len() as u64,
        deleted: report.deleted.clone(),
        skipped: report.skipped.clone(),
        unaged: report.unaged.clone(),
        pruned_records: report
            .pruned_records
            .iter()
            .map(|profile| profile.as_str().to_owned())
            .collect(),
        retired_images: report.retired_images.len() as u64,
        inert_reason: report.inert_reason.as_ref().map(token),
        finished_at_unix: report.finished_at_unix,
    }
}

pub(crate) fn event_view(model: &event::Model) -> EventView {
    EventView {
        id: model.id,
        occurred_at_unix: model.occurred_at_unix,
        kind: model.kind.clone(),
        allocation_id: model.allocation_id.clone(),
        vm_name: model.vm_name.clone(),
        profile: model.profile.clone(),
        actor: model.actor.clone(),
        payload: model.payload.clone(),
    }
}
