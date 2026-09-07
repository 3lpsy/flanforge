use flanforge_orm::UserRecord;
use flanforge_wire::WebuiUserInfo;

use crate::{WebuiFault, WebuiServices};

#[must_use]
pub fn info(user: &UserRecord) -> WebuiUserInfo {
    WebuiUserInfo {
        id: user.id,
        username: user.username.clone(),
        auth_source: user.auth_source.as_str().to_owned(),
        created_at_unix: user.created_at_unix,
    }
}

/// Lists accounts, never their credentials.
///
/// # Errors
///
/// Returns `Internal` on a store failure.
pub async fn handle(services: &WebuiServices) -> Result<Vec<WebuiUserInfo>, WebuiFault> {
    let users = services.users.list().await?;
    Ok(users.iter().map(info).collect())
}
