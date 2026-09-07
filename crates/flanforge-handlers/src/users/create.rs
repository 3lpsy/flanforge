use flanforge_wire::{CreateUserRequest, WebuiUserInfo};

use super::info;
use crate::{WebuiFault, WebuiServices};

/// Creates a password-backed account. The request DTO validated its shape at
/// extraction; this only hashes and stores.
///
/// # Errors
///
/// Returns `Conflict` when the name is taken or authdb is disabled.
pub async fn handle(
    services: &WebuiServices,
    request: &CreateUserRequest,
) -> Result<WebuiUserInfo, WebuiFault> {
    if !services.config.current().webui.authdb.enabled {
        return Err(WebuiFault::Conflict("password accounts are disabled"));
    }
    let hash =
        flanforge_webui_auth::hash_password(&request.password).map_err(|_| WebuiFault::Internal)?;
    let created = services
        .users
        .create_authdb_user(&request.username, &hash)
        .await?;
    tracing::info!(username = %created.username, "webui user created");
    Ok(info(&created))
}
