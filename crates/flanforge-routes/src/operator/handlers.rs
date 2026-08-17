use std::str::FromStr;

use axum::{
    Json,
    extract::{Path, State},
};
use flanforge_core::{Allocation, AllocationId};
use flanforge_manager::{AllocationSummary, OperatorStatus, SweepReport};
use serde::Deserialize;

use super::{super::error::ApiError, OperatorState};

/// Whether the sweep may delete. Absent or `false` plans only.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReapBody {
    delete: bool,
}

/// Lists what the service already knows about its allocations.
pub async fn list(State(state): State<OperatorState>) -> Json<Vec<AllocationSummary>> {
    Json(state.manager.list().await)
}

/// Cancels one allocation by ID, taking the lifecycle path the service uses.
///
/// # Errors
///
/// Returns a typed rejection for a malformed or unknown ID, or a lifecycle
/// failure while terminalizing.
pub async fn cancel(
    State(state): State<OperatorState>,
    Path(id): Path<String>,
) -> Result<Json<Allocation>, ApiError> {
    let id = AllocationId::from_str(&id).map_err(|_| ApiError::BadRequest)?;
    tracing::info!(allocation_id = %id, "operator allocation cancellation requested");
    Ok(Json(state.manager.cancel_by_id(id).await?))
}

/// Reports capacity, configuration generation, warm images, and the last sweep.
pub async fn status(State(state): State<OperatorState>) -> Json<OperatorStatus> {
    Json(state.manager.status_snapshot().await)
}

/// Plans a sweep, and deletes only when the body asks for it.
///
/// # Errors
///
/// Returns a typed rejection when the host cannot be listed or the image
/// records cannot be read.
pub async fn reap(
    State(state): State<OperatorState>,
    body: Option<Json<ReapBody>>,
) -> Result<Json<SweepReport>, ApiError> {
    let is_delete = body.is_some_and(|Json(body)| body.delete);
    tracing::info!(is_delete, "operator sweep requested");
    Ok(Json(state.manager.sweep(!is_delete).await?))
}
