use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use flanforge_extractors::{Body, RequireUser};
use flanforge_handlers::WebuiServices;
use flanforge_wire::{HotRetireRequest, ReapRequest};

use crate::fault_response;

/// Cancels one allocation; the actor lands in the audit log.
pub async fn cancel_allocation(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Path(id): Path<String>,
) -> Response {
    match flanforge_handlers::allocations::cancel(&services, &actor.session.user.username, &id)
        .await
    {
        Ok(view) => Json(view).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Drains or evicts one hot guest.
pub async fn hot_retire(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Path(name): Path<String>,
    Body(request): Body<HotRetireRequest>,
) -> Response {
    match flanforge_handlers::hot::retire(&services, &actor.session.user.username, &name, request)
        .await
    {
        Ok(guests) => Json(guests).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Plans a sweep, deleting only when asked.
pub async fn reap(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Body(request): Body<ReapRequest>,
) -> Response {
    match flanforge_handlers::reaper::handle(&services, &actor.session.user.username, request).await
    {
        Ok(report) => Json(report).into_response(),
        Err(fault) => fault_response(&fault),
    }
}
