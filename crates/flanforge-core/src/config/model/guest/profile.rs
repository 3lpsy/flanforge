use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::HotConfig;
use crate::{RepositoryName, RunnerLabel, VmName};

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
    pub allowed_refs: BTreeSet<String>,
    #[serde(default)]
    pub require_protected_ref: bool,
    #[serde(default)]
    pub network: NetworkMode,
    pub cpu_count: u8,
    pub memory_mb: u32,
    #[serde(default = "default_profile_storage_mb")]
    pub storage_mb: u64,
    pub boot_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub job_timeout_seconds: u64,
    pub cleanup_timeout_seconds: u64,
    #[serde(default)]
    pub warm_template: Option<VmName>,
    #[serde(default)]
    pub regeneration_workflow: Option<String>,
    /// Absent is hot off, the one default the operator does not get to change.
    #[serde(default)]
    pub hot: Option<HotConfig>,
    #[serde(default = "default_reap")]
    pub reap: bool,
}

const fn default_reap() -> bool {
    true
}

const fn default_profile_storage_mb() -> u64 {
    40_960
}
