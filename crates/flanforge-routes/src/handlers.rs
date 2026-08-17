use std::{str::FromStr, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use flanforge_core::{Allocation, AllocationId, AllocationState, ForgejoClaims};
use serde::Serialize;

use flanforge_auth::{TokenVerifier, bearer_token};
use flanforge_manager::{AllocationManager, CreateAllocation};

use super::{
    error::ApiError,
    input::{CreateBody, ValidatedJson},
};

#[derive(Clone)]
pub struct AppState {
    manager: AllocationManager,
    verifier: Arc<dyn TokenVerifier>,
    allocation_wait: Duration,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    #[must_use]
    pub fn new(
        manager: AllocationManager,
        verifier: Arc<dyn TokenVerifier>,
        allocation_wait: Duration,
    ) -> Self {
        Self {
            manager,
            verifier,
            allocation_wait,
        }
    }
}

pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Authenticates, validates, and creates an allocation.
///
/// # Errors
///
/// Returns a typed API rejection for invalid authentication, policy denial,
/// capacity, lifecycle, persistence, or wait-time failures.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(body): ValidatedJson<CreateBody>,
) -> Result<(StatusCode, Json<Allocation>), ApiError> {
    let claims = authenticate(&state, &headers).await?;
    let (request, options) = body.into_parts()?;
    tracing::info!(
        repository = %request.repository,
        profile = %request.profile,
        run_id = request.run_id,
        run_attempt = request.run_attempt,
        "authenticated allocation requested"
    );
    let created = state.manager.create(request, options, &claims).await?;
    let status = if created.is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let allocation_id = created.allocation.id;
    let allocation = match wait_until_ready(created, state.allocation_wait).await {
        Ok(allocation) => allocation,
        Err(ApiError::Timeout) => {
            tracing::warn!(%allocation_id, "allocation request timed out; cancelling worker");
            let _ = state
                .manager
                .cancel_authorized(allocation_id, &claims)
                .await;
            return Err(ApiError::Timeout);
        }
        Err(error) => return Err(error),
    };
    Ok((status, Json(allocation)))
}

/// Returns an allocation after authenticating and reauthorizing the caller.
///
/// # Errors
///
/// Returns a typed API rejection for invalid identity, allocation ID, policy,
/// or allocation state.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Allocation>, ApiError> {
    let claims = authenticate(&state, &headers).await?;
    let id = AllocationId::from_str(&id).map_err(|_| ApiError::BadRequest)?;
    tracing::debug!(allocation_id = %id, "authorized allocation status requested");
    Ok(Json(state.manager.get_authorized(id, &claims).await?))
}

/// Cancels an allocation after authenticating and reauthorizing the caller.
///
/// # Errors
///
/// Returns a typed API rejection for invalid identity, allocation ID, policy,
/// or allocation state.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Allocation>, ApiError> {
    let claims = authenticate(&state, &headers).await?;
    let id = AllocationId::from_str(&id).map_err(|_| ApiError::BadRequest)?;
    tracing::info!(allocation_id = %id, "authorized allocation cancellation requested");
    Ok(Json(state.manager.cancel_authorized(id, &claims).await?))
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<ForgejoClaims, ApiError> {
    let token = bearer_token(headers)?;
    state.verifier.verify(token).await.map_err(Into::into)
}

async fn wait_until_ready(
    mut created: CreateAllocation,
    timeout: Duration,
) -> Result<Allocation, ApiError> {
    tokio::time::timeout(timeout, async {
        loop {
            let allocation = created.receiver.borrow_and_update().clone();
            match allocation.state {
                AllocationState::WaitingForJob
                | AllocationState::Ready
                | AllocationState::Running
                | AllocationState::Completed => {
                    return Ok(allocation);
                }
                AllocationState::Failed | AllocationState::Cancelled => {
                    return Err(ApiError::Worker);
                }
                _ => {}
            }
            created
                .receiver
                .changed()
                .await
                .map_err(|_| ApiError::Internal)?;
        }
    })
    .await
    .map_err(|_| ApiError::Timeout)?
}

#[derive(Debug, Serialize)]
pub struct Health {
    status: &'static str,
    version: &'static str,
}
