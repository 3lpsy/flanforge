use axum::{
    Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use flanforge_handlers::WebuiServices;
use serde::Deserialize;

use crate::fault_response;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AllocationFilter {
    state: Option<String>,
    profile: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryQuery {
    state: Option<String>,
    profile: Option<String>,
    limit: Option<u64>,
    before: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventsQuery {
    kind: Option<String>,
    limit: Option<u64>,
    before: Option<i64>,
}

/// The dashboard document.
pub async fn status(State(services): State<WebuiServices>) -> Response {
    Json(flanforge_handlers::status::handle(&services).await).into_response()
}

/// Live allocations, optionally filtered.
pub async fn allocations(
    State(services): State<WebuiServices>,
    Query(filter): Query<AllocationFilter>,
) -> Response {
    Json(
        flanforge_handlers::allocations::list(
            &services,
            filter.state.as_deref(),
            filter.profile.as_deref(),
        )
        .await,
    )
    .into_response()
}

/// Durable allocation history, keyset-paged.
pub async fn allocation_history(
    State(services): State<WebuiServices>,
    Query(query): Query<HistoryQuery>,
) -> Response {
    match flanforge_handlers::allocations::history(
        &services,
        query.profile.as_deref(),
        query.state.as_deref(),
        query.limit,
        query.before.as_deref(),
    )
    .await
    {
        Ok(page) => Json(page).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// One allocation: record, live projection, and event timeline.
pub async fn allocation_detail(
    State(services): State<WebuiServices>,
    Path(id): Path<String>,
) -> Response {
    match flanforge_handlers::allocations::detail(&services, &id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Every hot guest record.
pub async fn hot(State(services): State<WebuiServices>) -> Response {
    Json(flanforge_handlers::hot::list(&services).await).into_response()
}

/// Every profile's warm image, as the status snapshot reports it.
pub async fn warm(State(services): State<WebuiServices>) -> Response {
    Json(
        flanforge_handlers::status::handle(&services)
            .await
            .warm_images,
    )
    .into_response()
}

/// The last sweep this process ran, or null.
pub async fn sweeps(State(services): State<WebuiServices>) -> Response {
    Json(
        flanforge_handlers::status::handle(&services)
            .await
            .last_sweep,
    )
    .into_response()
}

/// The durable event log, newest first.
pub async fn events(
    State(services): State<WebuiServices>,
    Query(query): Query<EventsQuery>,
) -> Response {
    match flanforge_handlers::events::handle(
        &services,
        query.kind.as_deref(),
        query.limit,
        query.before,
    )
    .await
    {
        Ok(page) => Json(page).into_response(),
        Err(fault) => fault_response(&fault),
    }
}
