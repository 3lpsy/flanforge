use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::Response,
};
use flanforge_orm::SessionRecord;

use super::error_response;

/// A resolved login: the account and the hash of the cookie that proved it,
/// so "sign out my other sessions" can name the one to keep.
#[derive(Clone, Debug)]
pub struct ResolvedSession {
    pub session: SessionRecord,
    pub token_hash: String,
}

/// What the identity middleware resolved for this request. Attached as an
/// extension so the guards and every extractor read one answer.
#[derive(Clone, Debug)]
pub struct WebuiIdentity(pub Option<ResolvedSession>);

/// The caller's identity, signed in or not. Missing middleware is a wiring
/// bug and fails closed as unauthenticated, loudly.
#[derive(Debug)]
pub struct Identity(pub Option<ResolvedSession>);

impl<S: Send + Sync> FromRequestParts<S> for Identity {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(identity) = parts.extensions.get::<WebuiIdentity>() {
            return Ok(Self(identity.0.clone()));
        }
        tracing::error!("webui identity middleware missing; failing closed");
        Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authentication required",
        ))
    }
}

/// A signed-in caller; anything else is a 401 before the handler runs.
#[derive(Debug)]
pub struct RequireUser(pub ResolvedSession);

impl<S: Send + Sync> FromRequestParts<S> for RequireUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Identity(identity) = Identity::from_request_parts(parts, state).await?;
        identity
            .map(Self)
            .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "authentication required"))
    }
}
