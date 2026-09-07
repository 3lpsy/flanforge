use super::{Config, ConfigError, HotConfig};

// Sanity bounds, now recorded in HOT_VM.md's Bounds table. The reset-gate
// range reuses `cleanup_timeout_seconds`, which bounds the same kind of
// guest operation.
const MAX_LIFETIME_SECONDS: (u64, u64) = (300, 604_800);
const IDLE_TTL_SECONDS: (u64, u64) = (30, 86_400);
const RESET_TIMEOUT_SECONDS: (u64, u64) = (5, 600);
/// Zero is admitted: a machine allowed no jobs and a pool holding no idle
/// machines are the same "configured to do nothing" mistake, and `max_idle`
/// has only ever warned about it. Both warn; see `ConfigAdvisory`.
const MAX_JOBS: (u32, u32) = (0, 1_000);

/// Refuses only what cannot be satisfied: a global cap above the host's own
/// slot count, and a bound outside its range. Everything else hot can get
/// wrong is the operator's call and surfaces through `Config::advisories`.
pub(super) fn ensure_hot_valid(config: &Config) -> Result<(), ConfigError> {
    if config.runtime.max_hot_vms > config.runtime.max_running_vms {
        return Err(ConfigError::OutOfRange {
            field: "runtime.max_hot_vms",
        });
    }
    for (name, profile) in &config.profiles {
        // Only an enabled table is refused. A disabled one bounds nothing the
        // daemon will act on, and failing the load over it would block the
        // very edit that switches hot off.
        if let Some(hot) = profile.hot.filter(|hot| hot.enabled) {
            ensure_bounds(&hot).map_err(|message| ConfigError::InvalidProfile {
                profile: name.to_string(),
                message: message.to_owned(),
            })?;
        }
    }
    Ok(())
}

fn ensure_bounds(hot: &HotConfig) -> Result<(), &'static str> {
    if !(MAX_LIFETIME_SECONDS.0..=MAX_LIFETIME_SECONDS.1).contains(&hot.max_lifetime_seconds) {
        return Err("hot.max_lifetime_seconds is outside the allowed range");
    }
    if !(IDLE_TTL_SECONDS.0..=IDLE_TTL_SECONDS.1).contains(&hot.idle_ttl_seconds) {
        return Err("hot.idle_ttl_seconds is outside the allowed range");
    }
    if !(RESET_TIMEOUT_SECONDS.0..=RESET_TIMEOUT_SECONDS.1).contains(&hot.reset_timeout_seconds) {
        return Err("hot.reset_timeout_seconds is outside the allowed range");
    }
    if !(MAX_JOBS.0..=MAX_JOBS.1).contains(&hot.max_jobs) {
        return Err("hot.max_jobs is outside the allowed range");
    }
    Ok(())
}
