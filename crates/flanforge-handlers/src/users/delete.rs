use flanforge_orm::SessionRecord;

use crate::{WebuiFault, WebuiServices};

/// Deletes an account; its sessions cascade. Deleting yourself is refused —
/// an admin must not saw off the branch they stand on mid-request.
///
/// # Errors
///
/// Returns `Forbidden` for self-deletion or `NotFound` for an unknown id.
pub async fn handle(
    services: &WebuiServices,
    actor: &SessionRecord,
    user_id: i64,
) -> Result<(), WebuiFault> {
    if actor.user.id == user_id {
        return Err(WebuiFault::Forbidden);
    }
    services.users.delete(user_id).await?;
    tracing::info!(user_id, actor = %actor.user.username, "webui user deleted");
    Ok(())
}
