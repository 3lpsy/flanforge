use axum::{
    extract::{FromRequestParts, Request, State},
    http::request::Parts,
    middleware::Next,
    response::Response,
};
use flanforge_auth::bearer_token;
use flanforge_core::ForgejoClaims;

use super::{error::ApiError, handlers::AppState};

/// Verifies the workflow credential once for the whole protected branch and
/// hands the verified claims on as a request extension. It runs ahead of every
/// body extractor, so an unauthenticated caller learns nothing from validation.
///
/// # Errors
///
/// Returns an authentication rejection when the credential is absent,
/// malformed, or unverifiable, and a provider rejection when OIDC is down.
pub async fn ensure_authenticated(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let claims = state
        .verifier()
        .verify(bearer_token(request.headers())?)
        .await?;
    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

/// The verified workflow identity. Taking it is how a handler declares itself
/// protected: the body cannot run without claims. It carries authentication
/// only — every per-profile authorization check still runs in the manager.
#[derive(Clone, Debug)]
pub struct Authenticated(pub ForgejoClaims);

impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Reached only if a claims-taking route escaped `ensure_authenticated`,
        // which is a router assembly defect; refuse rather than serve it.
        let Some(claims) = parts.extensions.get::<ForgejoClaims>().cloned() else {
            tracing::error!(
                path = %parts.uri.path(),
                "protected route was reached without verified claims"
            );
            return Err(ApiError::Authentication);
        };
        Ok(Self(claims))
    }
}
