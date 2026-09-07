use flanforge_orm::UserRecord;
use flanforge_webui_auth::{SessionToken, mint_session_token, verify_password_or_dummy};
use flanforge_wire::LoginRequest;

use crate::{WebuiFault, WebuiServices};

/// A successful login: the raw token for the cookie and who signed in. The
/// transport builds the `Set-Cookie`; the token never touches the database.
#[derive(Debug)]
pub struct IssuedSession {
    pub token: SessionToken,
    pub user: UserRecord,
    pub ttl_seconds: u64,
}

/// Password login. Every failure is the same refusal, and an unknown name
/// costs a real verification, so probing learns nothing.
///
/// # Errors
///
/// Returns `Conflict` when password login is disabled, `Unauthenticated` for
/// bad credentials, or `Internal` on a store failure.
pub async fn handle(
    services: &WebuiServices,
    request: &LoginRequest,
) -> Result<IssuedSession, WebuiFault> {
    let webui = services.config.current().webui.clone();
    if !webui.authdb.enabled {
        return Err(WebuiFault::Conflict("password login is disabled"));
    }
    let credentials = services
        .users
        .find_credentials(request.username.trim())
        .await?;
    let stored_hash = credentials.as_ref().and_then(|(_, hash)| hash.as_deref());
    let is_verified = verify_password_or_dummy(&request.password, stored_hash);
    let Some((user, _)) = credentials else {
        return Err(WebuiFault::Unauthenticated);
    };
    if !is_verified {
        tracing::warn!(username = %user.username, "webui login rejected");
        return Err(WebuiFault::Unauthenticated);
    }
    issue_for_user(services, user).await
}

/// Mints and persists a session for an already-authenticated account; the
/// password and OIDC paths converge here.
pub(crate) async fn issue_for_user(
    services: &WebuiServices,
    user: UserRecord,
) -> Result<IssuedSession, WebuiFault> {
    let ttl_seconds = services.config.current().webui.session_ttl_seconds;
    let token = mint_session_token().map_err(|_| WebuiFault::Internal)?;
    services
        .sessions
        .insert(
            &token.token_hash,
            user.id,
            user.auth_source.as_str(),
            ttl_seconds,
        )
        .await?;
    tracing::info!(username = %user.username, source = user.auth_source.as_str(), "webui login");
    Ok(IssuedSession {
        token,
        user,
        ttl_seconds,
    })
}
