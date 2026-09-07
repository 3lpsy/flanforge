use axum::{
    Json,
    extract::{Path, State},
};
use flanforge_orm::{UserError, UserRecord};
use flanforge_wire::{WEBUI_MAX_PASSWORD_BYTES, WEBUI_MIN_PASSWORD_BYTES, WebuiUserInfo};
use serde::Deserialize;

use super::{super::error::ApiError, OperatorState};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUserBody {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPasswordBody {
    password: String,
}

fn info(user: &UserRecord) -> WebuiUserInfo {
    WebuiUserInfo {
        id: user.id,
        username: user.username.clone(),
        auth_source: user.auth_source.as_str().to_owned(),
        created_at_unix: user.created_at_unix,
    }
}

fn ensure_username(value: &str) -> Result<(), ApiError> {
    let is_valid = (1..=64).contains(&value.len())
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if is_valid {
        Ok(())
    } else {
        Err(ApiError::BadRequest)
    }
}

fn hash_valid_password(value: &str) -> Result<String, ApiError> {
    if !(WEBUI_MIN_PASSWORD_BYTES..=WEBUI_MAX_PASSWORD_BYTES).contains(&value.len()) {
        return Err(ApiError::BadRequest);
    }
    flanforge_webui_auth::hash_password(value).map_err(|_| ApiError::Internal)
}

/// Lists web UI accounts, never their credentials.
///
/// # Errors
///
/// Returns a typed rejection on a store failure.
pub async fn webui_users_list(
    State(state): State<OperatorState>,
) -> Result<Json<Vec<WebuiUserInfo>>, ApiError> {
    let users = state.users.list().await?;
    Ok(Json(users.iter().map(info).collect()))
}

/// Creates a password-backed account; the bootstrap path for a fresh install.
///
/// # Errors
///
/// Returns a typed rejection for an invalid name or password, or a duplicate.
pub async fn webui_user_create(
    State(state): State<OperatorState>,
    Json(body): Json<CreateUserBody>,
) -> Result<Json<WebuiUserInfo>, ApiError> {
    ensure_username(&body.username)?;
    let hash = hash_valid_password(&body.password)?;
    let created = state
        .users
        .create_authdb_user(&body.username, &hash)
        .await?;
    tracing::info!(username = %created.username, "webui user created");
    Ok(Json(info(&created)))
}

/// Deletes an account by name; its sessions go with it.
///
/// # Errors
///
/// Returns a typed rejection for an unknown name.
pub async fn webui_user_delete(
    State(state): State<OperatorState>,
    Path(username): Path<String>,
) -> Result<Json<Vec<WebuiUserInfo>>, ApiError> {
    ensure_username(&username)?;
    let target = find_by_username(&state, &username).await?;
    state.users.delete(target.id).await?;
    tracing::info!(username = %username, "webui user deleted");
    webui_users_list(State(state)).await
}

/// Replaces a password and signs out every session of that account.
///
/// # Errors
///
/// Returns a typed rejection for an unknown name, an invalid password, or a
/// provider-managed account.
pub async fn webui_user_reset_password(
    State(state): State<OperatorState>,
    Path(username): Path<String>,
    Json(body): Json<SetPasswordBody>,
) -> Result<Json<WebuiUserInfo>, ApiError> {
    ensure_username(&username)?;
    let hash = hash_valid_password(&body.password)?;
    let target = find_by_username(&state, &username).await?;
    state.users.set_password_hash(target.id, &hash).await?;
    state.sessions.delete_for_user(target.id, None).await?;
    tracing::info!(username = %username, "webui user password reset; sessions signed out");
    Ok(Json(info(&target)))
}

async fn find_by_username(state: &OperatorState, username: &str) -> Result<UserRecord, ApiError> {
    state
        .users
        .find_credentials(username)
        .await?
        .map(|(user, _)| user)
        .ok_or(ApiError::UnknownUser)
}

impl From<UserError> for ApiError {
    fn from(error: UserError) -> Self {
        match error {
            UserError::Db(message) => {
                tracing::error!(%message, "webui user store failed");
                Self::Internal
            }
            UserError::UsernameTaken => Self::Conflict("username is already taken"),
            UserError::NotFound => Self::UnknownUser,
            UserError::ProviderManaged => {
                Self::Conflict("account is managed by the identity provider")
            }
        }
    }
}
