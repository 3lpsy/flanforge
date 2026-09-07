use serde::{Deserialize, Serialize};

/// The login page's bootstrap document: what auth is available and who, if
/// anyone, is signed in. One call drives the SPA's navbar and route guards.
// Independent capability flags the client renders from; not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebuiMeta {
    /// Blank for anonymous callers on a private deployment.
    pub version: String,
    /// Blank for anonymous callers on a private deployment.
    pub backend: String,
    pub authdb_enabled: bool,
    pub oidc_enabled: bool,
    pub public_read_only: bool,
    /// True while authdb is on and no account exists: the login page shows
    /// the `flanforged webui user add` hint instead of a doomed form.
    pub needs_bootstrap: bool,
    pub user: Option<WebuiSessionUser>,
}

/// The signed-in identity as pages render it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebuiSessionUser {
    pub id: i64,
    pub username: String,
    pub auth_source: String,
}
