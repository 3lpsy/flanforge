use flanforge_webui_auth::oidc::AuthorizeRedirect;

use super::login::issue_for_user;
use crate::{WebuiFault, WebuiServices, session::IssuedSession};

/// Starts the provider flow; 404 when OIDC is not configured.
///
/// # Errors
///
/// Returns `NotFound` when disabled or `Conflict` when the provider is down.
pub async fn begin(services: &WebuiServices) -> Result<AuthorizeRedirect, WebuiFault> {
    let relying_party = services.oidc.as_ref().ok_or(WebuiFault::NotFound)?;
    relying_party.begin_login().await.map_err(|error| {
        tracing::warn!(%error, "OIDC login could not start");
        WebuiFault::Conflict("identity provider unavailable")
    })
}

/// Finishes the provider flow: verifies the callback, provisions the account
/// on first sight, and mints an ordinary session.
///
/// # Errors
///
/// Returns `Unauthenticated` for any broken leg; the specifics are logged.
pub async fn finish(
    services: &WebuiServices,
    code: &str,
    state: &str,
    state_cookie: &str,
) -> Result<IssuedSession, WebuiFault> {
    let relying_party = services.oidc.as_ref().ok_or(WebuiFault::NotFound)?;
    let identity = relying_party
        .finish_login(code, state, state_cookie)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "OIDC login rejected");
            WebuiFault::Unauthenticated
        })?;
    let user = services
        .users
        .provision_oidc_user(&identity.subject, &identity.preferred_username)
        .await?;
    issue_for_user(services, user).await
}
