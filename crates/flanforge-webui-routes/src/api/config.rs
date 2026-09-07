use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use flanforge_extractors::{Body, RequireUser};
use flanforge_handlers::WebuiServices;
use flanforge_wire::ConfigUpdate;

use crate::fault_response;

/// The editable configuration view. Mutating tier: the file carries
/// authorization policy and infrastructure paths.
pub async fn get(State(services): State<WebuiServices>) -> Response {
    match flanforge_handlers::config::get(&services).await {
        Ok(view) => Json(view).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Applies allowlisted changes; the actor lands in the audit log.
pub async fn patch(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Body(request): Body<ConfigUpdate>,
) -> Response {
    match flanforge_handlers::config::update(&services, &actor.session.user.username, &request)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Creates a whole profile; the actor lands in the audit log.
pub async fn profile_put(
    State(services): State<WebuiServices>,
    axum::extract::Path(name): axum::extract::Path<String>,
    RequireUser(actor): RequireUser,
    Body(request): Body<flanforge_wire::ProfileCreate>,
) -> Response {
    match flanforge_handlers::config::profile_create(
        &services,
        &actor.session.user.username,
        &name,
        &request,
    )
    .await
    {
        Ok(result) => (axum::http::StatusCode::CREATED, Json(result)).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

/// Removes a profile; hot machines it still holds are reported, then drained.
pub async fn profile_delete(
    State(services): State<WebuiServices>,
    axum::extract::Path(name): axum::extract::Path<String>,
    RequireUser(actor): RequireUser,
    Body(request): Body<flanforge_wire::ProfileDelete>,
) -> Response {
    match flanforge_handlers::config::profile_delete(
        &services,
        &actor.session.user.username,
        &name,
        &request,
    )
    .await
    {
        Ok(result) => Json(result).into_response(),
        Err(fault) => fault_response(&fault),
    }
}
