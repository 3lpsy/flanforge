use serde::{Deserialize, Serialize};

use crate::hot::HotLanePolicy;

/// How far the recycle gate resets simulator state. `None` lets a job see the
/// previous job's app data; `Erase` gives back most of what hot saves.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulatorReset {
    None,
    #[default]
    Apps,
    Erase,
}

/// Per-profile hot-guest policy. The table is optional; its absence is hot off,
/// which is the one default that is not the operator's to change. Every key
/// inside it has a default, so a partially-written table is still fully bounded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HotConfig {
    // No `default_enabled()` helper on purpose: `bool::default()` is false and
    // there is no way to spell the opposite.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub lanes: HotLanePolicy,
    #[serde(default = "default_max_idle")]
    pub max_idle: u8,
    #[serde(default = "default_max_lifetime_seconds")]
    pub max_lifetime_seconds: u64,
    #[serde(default = "default_max_jobs")]
    pub max_jobs: u32,
    #[serde(default = "default_idle_ttl_seconds")]
    pub idle_ttl_seconds: u64,
    #[serde(default = "default_reset_timeout_seconds")]
    pub reset_timeout_seconds: u64,
    #[serde(default)]
    pub simulator_reset: SimulatorReset,
}

impl Default for HotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lanes: HotLanePolicy::default(),
            max_idle: default_max_idle(),
            max_lifetime_seconds: default_max_lifetime_seconds(),
            max_jobs: default_max_jobs(),
            idle_ttl_seconds: default_idle_ttl_seconds(),
            reset_timeout_seconds: default_reset_timeout_seconds(),
            simulator_reset: SimulatorReset::default(),
        }
    }
}

const fn default_max_idle() -> u8 {
    1
}

const fn default_max_lifetime_seconds() -> u64 {
    14_400
}

const fn default_max_jobs() -> u32 {
    20
}

const fn default_idle_ttl_seconds() -> u64 {
    900
}

const fn default_reset_timeout_seconds() -> u64 {
    120
}
