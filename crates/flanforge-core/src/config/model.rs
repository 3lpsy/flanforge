use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ProfileName, RepositoryName, RunnerLabel, VmName, VmPrefix};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub logging: LoggingConfig,
    pub server: ServerConfig,
    pub oidc: OidcConfig,
    pub forgejo: ForgejoConfig,
    pub runtime: RuntimeConfig,
    pub guest: GuestConfig,
    #[serde(default)]
    pub tailscale: TailscaleConfig,
    pub profiles: BTreeMap<ProfileName, Profile>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub state_dir: PathBuf,
    pub tart_path: PathBuf,
    pub ssh_path: PathBuf,
    pub scp_path: PathBuf,
    pub forgejo_runner_host_path: PathBuf,
    pub vm_prefix: VmPrefix,
    pub tart_home: Option<PathBuf>,
    #[serde(default = "default_max_running_vms")]
    pub max_running_vms: u8,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    #[serde(default = "default_reap_interval_hours")]
    pub reap_interval_hours: u64,
    #[serde(default)]
    pub host_cpu_count: Option<u8>,
    #[serde(default)]
    pub host_memory_mb: Option<u32>,
}

const fn default_max_running_vms() -> u8 {
    2
}

const fn default_poll_seconds() -> u64 {
    2
}

const fn default_reap_interval_hours() -> u64 {
    168
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestConfig {
    pub ssh_user: String,
    pub ssh_identity_file: PathBuf,
    /// Pinned host-key anchor; read only while `verify_host_key` is enabled.
    #[serde(default)]
    pub ssh_known_hosts_file: Option<PathBuf>,
    #[serde(default)]
    pub ssh_host_key_alias: Option<String>,
    pub forgejo_runner_path: PathBuf,
    #[serde(default = "default_ssh_connect_timeout_seconds")]
    pub ssh_connect_timeout_seconds: u64,
    /// Disabling this exposes the host-to-guest channel to an on-path attacker.
    #[serde(default = "default_verify_host_key")]
    pub verify_host_key: bool,
}

const fn default_ssh_connect_timeout_seconds() -> u64 {
    5
}

const fn default_verify_host_key() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TailscaleConfig {
    pub enabled: bool,
    pub preauth_key_file: Option<PathBuf>,
    pub login_server: Option<Url>,
    pub hostname: Option<String>,
    pub extra_args: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Default,
    Softnet,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub repository: RepositoryName,
    pub template: VmName,
    pub runner_label: RunnerLabel,
    pub job_name: String,
    pub allowed_workflows: BTreeSet<String>,
    pub allowed_events: BTreeSet<String>,
    #[serde(default)]
    pub allowed_refs: BTreeSet<String>,
    #[serde(default)]
    pub allowed_ref_prefixes: BTreeSet<String>,
    #[serde(default)]
    pub require_protected_ref: bool,
    #[serde(default)]
    pub network: NetworkMode,
    pub cpu_count: u8,
    pub memory_mb: u32,
    pub boot_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub job_timeout_seconds: u64,
    pub cleanup_timeout_seconds: u64,
    /// Warm image consumers clone; unset disables warming for this project.
    #[serde(default)]
    pub warm_template: Option<VmName>,
    /// The only workflow whose allocation may produce that image.
    #[serde(default)]
    pub regeneration_workflow: Option<String>,
    #[serde(default = "default_reap")]
    pub reap: bool,
}

const fn default_reap() -> bool {
    true
}
