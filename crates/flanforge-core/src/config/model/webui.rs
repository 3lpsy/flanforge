use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebuiConfig {
    pub enabled: bool,
    pub public_read_only: bool,
    pub session_ttl_seconds: u64,
    pub request_body_limit_bytes: usize,
    /// Development override: serve the UI from this directory instead of the
    /// assets embedded at build time.
    pub dev_dist_dir: Option<PathBuf>,
    pub authdb: WebuiAuthdbConfig,
    pub oidc: WebuiOidcConfig,
}

impl Default for WebuiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            public_read_only: false,
            session_ttl_seconds: 604_800,
            request_body_limit_bytes: 65_536,
            dev_dist_dir: None,
            authdb: WebuiAuthdbConfig::default(),
            oidc: WebuiOidcConfig::default(),
        }
    }
}

/// Password login against the daemon's own users table. Disable to run the
/// web UI on OIDC alone.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebuiAuthdbConfig {
    pub enabled: bool,
}

impl Default for WebuiAuthdbConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// OIDC login for the web UI: the daemon is the relying party in a standard
/// authorization-code flow. Unrelated to `[oidc]`, which verifies workflow
/// tokens minted by the git server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebuiOidcConfig {
    pub enabled: bool,
    pub issuer: Option<Url>,
    pub client_id: Option<String>,
    pub client_secret_file: Option<PathBuf>,
    pub redirect_url: Option<Url>,
    pub scopes: Vec<String>,
}

impl Default for WebuiOidcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            issuer: None,
            client_id: None,
            client_secret_file: None,
            redirect_url: None,
            scopes: vec![
                "openid".to_owned(),
                "profile".to_owned(),
                "email".to_owned(),
            ],
        }
    }
}
