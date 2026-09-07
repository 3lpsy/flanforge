use serde::{Deserialize, Serialize};

/// Password length policy, one place for the CLI, the operator surface, and
/// the web UI to agree on.
pub const WEBUI_MIN_PASSWORD_BYTES: usize = 8;
pub const WEBUI_MAX_PASSWORD_BYTES: usize = 128;

/// One web UI account as the API reports it. Never carries a credential.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebuiUserInfo {
    pub id: i64,
    pub username: String,
    /// `authdb` or `oidc`; provider-managed rows have no password.
    pub auth_source: String,
    pub created_at_unix: i64,
}
