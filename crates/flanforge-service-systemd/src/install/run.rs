use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_core::{Config, RuntimeBackendKind};
use flanforge_service_files::{atomic_write, install_current_binary};

use crate::{
    constants::{COMMAND_TIMEOUT, DEFAULT_USER, SERVICE_NAME},
    control::{effective_user, systemctl},
    paths::{SystemdPaths, is_present},
};

use super::{
    identity::{
        ensure_db_access, ensure_identity_access, ensure_root, ensure_service_user,
        ensure_state_access, make_configuration_readable,
    },
    unit::systemd_unit,
};

pub(crate) async fn install(config: &Config, config_path: &Path) -> Result<()> {
    if config.runtime.backend_kind() != RuntimeBackendKind::Libvirt {
        bail!("Linux systemd installation requires the libvirt runtime backend");
    }
    ensure_root().await?;
    let paths = SystemdPaths::default();
    let unit = systemd_unit(&paths, config_path, config)?;
    let has_existing_unit = is_present(&paths.definition).await?;
    if has_existing_unit {
        systemctl(&["daemon-reload"], COMMAND_TIMEOUT).await?;
    } else {
        ensure_service_user(&paths).await?;
    }
    let service_user = if has_existing_unit {
        effective_user().await?
    } else {
        DEFAULT_USER.to_owned()
    };

    let binary_parent = paths
        .binary
        .parent()
        .ok_or_else(|| anyhow::anyhow!("service binary has no parent"))?;
    tokio::fs::create_dir_all(binary_parent)
        .await
        .context("cannot create service binary directory")?;
    ensure_state_access(&service_user, &config.runtime.state_dir).await?;
    install_current_binary(&paths.binary, 0o755).await?;

    if has_existing_unit {
        let existing = tokio::fs::read(&paths.definition)
            .await
            .context("cannot read installed systemd unit")?;
        if existing != unit.as_bytes() {
            tracing::warn!(path = %paths.definition.display(), "preserving locally modified systemd unit");
        }
    } else {
        atomic_write(&paths.definition, unit.as_bytes(), 0o644).await?;
        make_configuration_readable(config_path).await?;
    }

    ensure_identity_access(&service_user, config_path, &config.runtime.state_dir).await?;
    if let Some(db_dir) = config.db.resolved_db_path(config_path).parent() {
        ensure_db_access(&service_user, db_dir).await?;
    }
    systemctl(&["daemon-reload"], COMMAND_TIMEOUT).await?;
    systemctl(&["enable", SERVICE_NAME], COMMAND_TIMEOUT).await?;
    tracing::info!(
        service = SERVICE_NAME,
        "systemd service installed; start it with `flanforged daemon start`"
    );
    Ok(())
}
