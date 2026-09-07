use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::ProfileName;

use super::{
    DbConfig, GuestConfig, Profile, RuntimeConfig, TailscaleConfig, WebuiConfig,
    guest::GuestDocument,
};

/// The resolved configuration. `ConfigDocument` below is the schema authority:
/// what is required, defaulted, or denied is declared there, not here.
#[derive(Clone, Debug, Serialize)]
pub struct Config {
    pub logging: LoggingConfig,
    pub server: ServerConfig,
    pub db: DbConfig,
    pub webui: WebuiConfig,
    pub oidc: OidcConfig,
    pub forgejo: ForgejoConfig,
    pub runtime: RuntimeConfig,
    pub guest: GuestConfig,
    pub tailscale: TailscaleConfig,
    pub profiles: BTreeMap<ProfileName, Profile>,
}

/// The document as written, and the whole schema: which tables are required,
/// which default, and that unknown keys are denied. Loosening anything here
/// loosens it everywhere, with nothing else to catch it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    #[serde(default)]
    logging: LoggingConfig,
    server: ServerConfig,
    #[serde(default)]
    db: DbConfig,
    #[serde(default)]
    webui: WebuiConfig,
    oidc: OidcConfig,
    forgejo: ForgejoConfig,
    runtime: RuntimeConfig,
    guest: GuestDocument,
    #[serde(default)]
    tailscale: TailscaleConfig,
    profiles: BTreeMap<ProfileName, Profile>,
}

/// Resolves `guest.channel` eagerly against the backend the same document
/// declares, so nothing downstream has to re-derive it or carry an unresolved
/// option. Each field deserializes into its own resolved type, so table order
/// in the file cannot change the outcome.
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let document = ConfigDocument::deserialize(deserializer)?;
        let guest = document.guest.resolve(&document.runtime.backend);
        Ok(Self {
            logging: document.logging,
            server: document.server,
            db: document.db,
            webui: document.webui,
            oidc: document.oidc,
            forgejo: document.forgejo,
            runtime: document.runtime,
            guest,
            tailscale: document.tailscale,
            profiles: document.profiles,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub path: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            path: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub request_body_limit_bytes: usize,
    pub allocation_wait_seconds: u64,
    pub shutdown_grace_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9_843),
            request_body_limit_bytes: 4_096,
            allocation_wait_seconds: 600,
            shutdown_grace_seconds: 30,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    pub issuer: Url,
    pub audience: String,
    pub jwks_url: Url,
    #[serde(default = "default_jwks_cache_seconds")]
    pub jwks_cache_seconds: u64,
    #[serde(default = "default_clock_skew_seconds")]
    pub clock_skew_seconds: u64,
}

const fn default_jwks_cache_seconds() -> u64 {
    300
}

const fn default_clock_skew_seconds() -> u64 {
    30
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgejoConfig {
    pub api_url: Url,
    pub api_token_file: PathBuf,
    #[serde(default = "default_http_timeout_seconds")]
    pub http_timeout_seconds: u64,
}

const fn default_http_timeout_seconds() -> u64 {
    15
}
