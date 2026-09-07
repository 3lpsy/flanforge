use flanforge_orm::SessionRecord;
use flanforge_wire::SetPasswordRequest;

use crate::{WebuiFault, WebuiServices};

/// Replaces an account's password and signs its sessions out — except the
/// acting session when the admin is changing their own.
///
/// # Errors
///
/// Returns `NotFound` for an unknown id or `Conflict` for a provider-managed
/// account.
pub async fn handle(
    services: &WebuiServices,
    actor: &SessionRecord,
    actor_token_hash: &str,
    user_id: i64,
    request: &SetPasswordRequest,
) -> Result<(), WebuiFault> {
    let hash =
        flanforge_webui_auth::hash_password(&request.password).map_err(|_| WebuiFault::Internal)?;
    services.users.set_password_hash(user_id, &hash).await?;
    let keep = (actor.user.id == user_id).then_some(actor_token_hash);
    services.sessions.delete_for_user(user_id, keep).await?;
    tracing::info!(user_id, actor = %actor.user.username, "webui password reset; sessions signed out");
    Ok(())
}
