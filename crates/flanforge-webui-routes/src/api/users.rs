use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use flanforge_extractors::{Body, RequireUser};
use flanforge_handlers::WebuiServices;
use flanforge_wire::{CreateUserRequest, SetPasswordRequest};

use crate::fault_response;

/// Lists accounts. Mutating tier: user enumeration is a sensitive read.
pub async fn list(State(services): State<WebuiServices>) -> Response {
    match flanforge_handlers::users::list(&services).await {
        Ok(users) => Json(users).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

pub async fn create(
    State(services): State<WebuiServices>,
    Body(request): Body<CreateUserRequest>,
) -> Response {
    match flanforge_handlers::users::create(&services, &request).await {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(fault) => fault_response(&fault),
    }
}

pub async fn delete(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Path(user_id): Path<i64>,
) -> Response {
    match flanforge_handlers::users::delete(&services, &actor.session, user_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(fault) => fault_response(&fault),
    }
}

pub async fn set_password(
    State(services): State<WebuiServices>,
    RequireUser(actor): RequireUser,
    Path(user_id): Path<i64>,
    Body(request): Body<SetPasswordRequest>,
) -> Response {
    match flanforge_handlers::users::set_password(
        &services,
        &actor.session,
        &actor.token_hash,
        user_id,
        &request,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(fault) => fault_response(&fault),
    }
}
