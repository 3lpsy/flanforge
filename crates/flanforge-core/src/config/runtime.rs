use super::{
    Config, ConfigError, NetworkMode, RuntimeBackendConfig,
    validate::{ensure_path, ensure_range, ensure_safe_name},
};

pub(super) fn ensure_runtime_valid(config: &Config) -> Result<(), ConfigError> {
    let runtime = &config.runtime;
    ensure_path("runtime.state_dir", &runtime.state_dir)?;
    ensure_path("runtime.ssh_path", &runtime.ssh_path)?;
    ensure_path("runtime.scp_path", &runtime.scp_path)?;
    ensure_backend_valid(config)?;
    runtime
        .vm_prefix
        .ensure_valid()
        .map_err(|_| ConfigError::UnsafeValue {
            field: "runtime.vm_prefix",
        })?;
    // A sanity bound, not a policy, and the same one for both backends: what
    // really bounds concurrent guests is `host_cpu_count` / `host_memory_mb` /
    // `host_storage_mb`, which `ensure_capacity` folds per allocation and which
    // fails closed when unset. The per-backend *defaults* still differ.
    ensure_range(
        "runtime.max_running_vms",
        &runtime.max_running_vms,
        1,
        u8::MAX,
    )?;
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
    ensure_tart_home_present(config)?;
    ensure_libvirt_warm_valid(config)
}

fn ensure_backend_valid(config: &Config) -> Result<(), ConfigError> {
    let runtime = &config.runtime;
    match &runtime.backend {
        RuntimeBackendConfig::Tart(tart) => {
            ensure_path("runtime.backend.path", &tart.path)?;
            if let Some(runner_host_path) = &tart.runner_host_path {
                ensure_path("runtime.backend.runner_host_path", runner_host_path)?;
            }
            if let Some(tart_home) = &tart.home {
                ensure_path("runtime.backend.home", tart_home)?;
            }
        }
        RuntimeBackendConfig::Libvirt(libvirt) => {
            if !flanforge_utils::is_supported_libvirt_uri(
                &libvirt.uri,
                libvirt.allow_insecure_transport,
            ) {
                return Err(ConfigError::UnsafeValue {
                    field: "runtime.backend.uri",
                });
            }
            ensure_safe_name("runtime.backend.pool", &libvirt.pool, 1, 80)?;
            ensure_safe_name("runtime.backend.network", &libvirt.network, 1, 80)?;
            ensure_path(
                "runtime.backend.image_manifest_dir",
                &libvirt.image_manifest_dir,
            )?;
            let state = flanforge_paths::libvirt_state_paths(&runtime.state_dir);
            let warm_records = flanforge_paths::warm_record_dir(&runtime.state_dir);
            // `published_bases` is deliberately absent: it is where the
            // manifests live, so overlapping it is the supported layout.
            for reserved in [
                &state.allocations,
                &state.imports,
                &state.service_instance,
                &state.warm,
                &warm_records,
            ] {
                if paths_overlap(&libvirt.image_manifest_dir, reserved) {
                    return Err(ConfigError::UnsafeValue {
                        field: "runtime.backend.image_manifest_dir",
                    });
                }
            }
            if let Some(import_dir) = &libvirt.image_import_dir {
                ensure_path("runtime.backend.image_import_dir", import_dir)?;
                // The scan reads artifacts; overlapping any state the daemon
                // writes would let an import consume its own byproducts.
                if paths_overlap(import_dir, &runtime.state_dir)
                    || paths_overlap(import_dir, &libvirt.image_manifest_dir)
                {
                    return Err(ConfigError::UnsafeValue {
                        field: "runtime.backend.image_import_dir",
                    });
                }
            }
            ensure_path("runtime.backend.qemu_img_path", &libvirt.qemu_img_path)?;
            ensure_path("runtime.backend.virsh_path", &libvirt.virsh_path)?;
            ensure_range(
                "runtime.backend.min_storage_free_mb",
                &libvirt.min_storage_free_mb,
                1_024,
                16_777_216,
            )?;
            ensure_range(
                "runtime.backend.warm_capture_timeout_seconds",
                &libvirt.warm_capture_timeout_seconds,
                60,
                7_200,
            )?;
            for (name, profile) in &config.profiles {
                if profile.cleanup_timeout_seconds < super::LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS {
                    return Err(ConfigError::InvalidProfile {
                        profile: name.to_string(),
                        message: "libvirt cleanup timeout cannot reserve helper completion slack"
                            .to_owned(),
                    });
                }
                if profile.network == NetworkMode::Softnet {
                    return Err(ConfigError::UnsupportedBackendCapability {
                        backend: "libvirt",
                        capability: "softnet networking",
                        profile: Some(name.to_string()),
                    });
                }
            }
        }
    }
    Ok(())
}

fn paths_overlap(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// The budget is whole, bounded, and able to admit every profile.
fn ensure_host_budget_valid(config: &Config) -> Result<(), ConfigError> {
    let runtime = &config.runtime;
    let is_libvirt = matches!(&runtime.backend, RuntimeBackendConfig::Libvirt(_));
    if runtime.host_cpu_count.is_none()
        && runtime.host_memory_mb.is_none()
        && runtime.host_storage_mb.is_none()
        && !is_libvirt
    {
        return Ok(());
    }
    if runtime.host_cpu_count.is_some() != runtime.host_memory_mb.is_some() {
        return Err(ConfigError::OutOfRange {
            field: if runtime.host_cpu_count.is_some() {
                "runtime.host_cpu_count"
            } else {
                "runtime.host_memory_mb"
            },
        });
    }
    let Some(cpu_count) = runtime.host_cpu_count else {
        return Err(ConfigError::OutOfRange {
            field: "runtime.host_cpu_count",
        });
    };
    let Some(memory_mb) = runtime.host_memory_mb else {
        return Err(ConfigError::OutOfRange {
            field: "runtime.host_memory_mb",
        });
    };
    if is_libvirt && runtime.host_storage_mb.is_none() {
        return Err(ConfigError::OutOfRange {
            field: "runtime.host_storage_mb",
        });
    }
    ensure_range("runtime.host_cpu_count", &cpu_count, 1, 255)?;
    ensure_range("runtime.host_memory_mb", &memory_mb, 2_048, 1_048_576)?;
    if let Some(storage_mb) = runtime.host_storage_mb {
        ensure_range("runtime.host_storage_mb", &storage_mb, 16_384, 16_777_216)?;
    }
    for (name, profile) in &config.profiles {
        let exceeds_reserved_storage = config.runtime.libvirt().is_some_and(|libvirt| {
            profile
                .storage_mb
                .checked_add(libvirt.min_storage_free_mb)
                .is_none_or(|required| {
                    runtime
                        .host_storage_mb
                        .is_none_or(|available| required > available)
                })
        });
        if profile.cpu_count > cpu_count
            || profile.memory_mb > memory_mb
            || runtime
                .host_storage_mb
                .is_some_and(|host| profile.storage_mb > host)
        {
            return Err(ConfigError::InvalidProfile {
                profile: name.to_string(),
                message: "profile is larger than the host budget and can never be admitted"
                    .to_owned(),
            });
        }
        if exceeds_reserved_storage {
            return Err(ConfigError::InvalidProfile {
                profile: name.to_string(),
                message: "profile plus the storage reserve is larger than the host budget and can never be admitted".to_owned(),
            });
        }
    }
    if let RuntimeBackendConfig::Libvirt(libvirt) = &runtime.backend {
        let host_storage_mb = runtime.host_storage_mb.unwrap_or_default();
        if libvirt.min_storage_free_mb >= host_storage_mb {
            return Err(ConfigError::OutOfRange {
                field: "runtime.backend.min_storage_free_mb",
            });
        }
    }
    Ok(())
}

/// The fingerprint stats the library path the daemon passes to Tart.
fn ensure_tart_home_present(config: &Config) -> Result<(), ConfigError> {
    if matches!(&config.runtime.backend, RuntimeBackendConfig::Tart(_))
        && config
            .runtime
            .tart()
            .is_some_and(|tart| tart.home.is_none())
        && config
            .profiles
            .values()
            .any(|profile| profile.warm_template.is_some())
    {
        return Err(ConfigError::MissingRuntimeSetting {
            field: "backend.home",
        });
    }
    Ok(())
}

/// Retirement of superseded generations lives in the reaper sweep, so a
/// disabled sweep means libvirt never reclaims one.
fn ensure_libvirt_warm_valid(config: &Config) -> Result<(), ConfigError> {
    if matches!(&config.runtime.backend, RuntimeBackendConfig::Libvirt(_))
        && config.runtime.reap_interval_hours == 0
        && config
            .profiles
            .values()
            .any(|profile| profile.warm_template.is_some())
    {
        return Err(ConfigError::MissingRuntimeSetting {
            field: "reap_interval_hours",
        });
    }
    Ok(())
}
