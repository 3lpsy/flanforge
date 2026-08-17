use super::{
    Config, ConfigError,
    validate::{ensure_path, ensure_range},
};

pub(super) fn ensure_runtime_valid(config: &Config) -> Result<(), ConfigError> {
    let runtime = &config.runtime;
    ensure_path("runtime.state_dir", &runtime.state_dir)?;
    ensure_path("runtime.tart_path", &runtime.tart_path)?;
    ensure_path("runtime.ssh_path", &runtime.ssh_path)?;
    ensure_path("runtime.scp_path", &runtime.scp_path)?;
    ensure_path(
        "runtime.forgejo_runner_host_path",
        &runtime.forgejo_runner_host_path,
    )?;
    if let Some(tart_home) = &runtime.tart_home {
        ensure_path("runtime.tart_home", tart_home)?;
    }
    runtime
        .vm_prefix
        .ensure_valid()
        .map_err(|_| ConfigError::UnsafeValue {
            field: "runtime.vm_prefix",
        })?;
    ensure_range("runtime.max_running_vms", &runtime.max_running_vms, 1, 2)?;
    ensure_range("runtime.poll_seconds", &runtime.poll_seconds, 1, 30)?;
    if runtime.reap_interval_hours != 0 {
        ensure_range(
            "runtime.reap_interval_hours",
            &runtime.reap_interval_hours,
            1,
            8_760,
        )?;
    }
    ensure_host_budget_valid(config)?;
    ensure_tart_home_present(config)
}

/// The budget is whole, bounded, and able to admit every profile.
fn ensure_host_budget_valid(config: &Config) -> Result<(), ConfigError> {
    let runtime = &config.runtime;
    match (runtime.host_cpu_count, runtime.host_memory_mb) {
        (None, None) => return Ok(()),
        (Some(_), None) => {
            return Err(ConfigError::OutOfRange {
                field: "runtime.host_cpu_count",
            });
        }
        (None, Some(_)) => {
            return Err(ConfigError::OutOfRange {
                field: "runtime.host_memory_mb",
            });
        }
        (Some(cpu_count), Some(memory_mb)) => {
            ensure_range("runtime.host_cpu_count", &cpu_count, 1, 255)?;
            ensure_range("runtime.host_memory_mb", &memory_mb, 2_048, 1_048_576)?;
            for (name, profile) in &config.profiles {
                if profile.cpu_count > cpu_count || profile.memory_mb > memory_mb {
                    return Err(ConfigError::InvalidProfile {
                        profile: name.to_string(),
                        message: "profile is larger than the host budget and can never be admitted"
                            .to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// The fingerprint stats the library path the daemon passes to Tart.
fn ensure_tart_home_present(config: &Config) -> Result<(), ConfigError> {
    if config.runtime.tart_home.is_none()
        && config
            .profiles
            .values()
            .any(|profile| profile.warm_template.is_some())
    {
        return Err(ConfigError::MissingRuntimeSetting { field: "tart_home" });
    }
    Ok(())
}
