use std::path::Path;

use anyhow::{Result, bail};
use flanforge_core::Config;
use flanforge_paths::SystemdPaths;

use crate::{
    constants::{DEFAULT_USER, KILL_PATH},
    unit_path::UnitPath,
};

pub(crate) fn systemd_unit(
    paths: &SystemdPaths,
    config_path: &Path,
    config: &Config,
) -> Result<String> {
    ensure_paths_visible_with_protect_home(config_path, config)?;
    let binary = UnitPath::new(&paths.binary)?.render();
    let state_dir = UnitPath::new(&config.runtime.state_dir)?.render();
    // The database lives beside the config by default, outside StateDirectory,
    // so its directory must be writable under ProtectSystem=strict.
    let db_dir = config
        .db
        .resolved_db_path(config_path)
        .parent()
        .map(|parent| UnitPath::new(parent).map(|path| path.render()))
        .transpose()?
        .unwrap_or_else(|| state_dir.clone());
    let writable_paths = if db_dir == state_dir {
        state_dir.clone()
    } else {
        format!("{state_dir} {db_dir}")
    };
    let config_path = UnitPath::new(config_path)?.render();
    let environment = UnitPath::new(&paths.environment)?.render();
    let kill = UnitPath::new(Path::new(KILL_PATH))?.render();
    let state_name = managed_directory_name(&paths.state_dir, Path::new("/var/lib"))?;
    let runtime_name = managed_directory_name(&paths.runtime_dir, Path::new("/run"))?;
    let timeout = config.server.shutdown_grace_seconds.saturating_add(5);
    Ok(format!(
        "[Unit]\nDescription=FlanForge ephemeral runner allocator\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nUser={DEFAULT_USER}\nUMask=0077\nEnvironmentFile=-{environment}\nExecStart={binary} --config {config_path} daemon run\nExecReload={kill} -HUP $MAINPID\nRestart=on-failure\nRestartSec=5s\nTimeoutStopSec={timeout}s\nStateDirectory={state_name}\nStateDirectoryMode=0750\nRuntimeDirectory={runtime_name}\nRuntimeDirectoryMode=0750\nReadWritePaths={writable_paths}\nNoNewPrivileges=true\nPrivateDevices=true\nPrivateTmp=true\nProtectSystem=strict\nProtectHome=true\nProtectControlGroups=true\nProtectKernelLogs=true\nProtectKernelModules=true\nProtectKernelTunables=true\nRestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\nRestrictSUIDSGID=true\nLockPersonality=true\nCapabilityBoundingSet=\nAmbientCapabilities=\n\n[Install]\nWantedBy=multi-user.target\n"
    ))
}

fn managed_directory_name<'a>(path: &'a Path, root: &Path) -> Result<&'a str> {
    if path.parent() != Some(root) {
        bail!("systemd managed directory is outside {}", root.display());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("systemd managed directory has no UTF-8 name"))?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("systemd managed directory name is invalid");
    }
    Ok(name)
}

fn ensure_paths_visible_with_protect_home(config_path: &Path, config: &Config) -> Result<()> {
    let db_path = config.db.resolved_db_path(config_path);
    let mut paths = vec![
        config_path,
        &db_path,
        &config.runtime.state_dir,
        &config.runtime.ssh_path,
        &config.runtime.scp_path,
        &config.forgejo.api_token_file,
    ];
    // Only a configured SSH identity has to stay visible: the agent channel
    // reaches the guest without one.
    if let Some(ssh) = config.guest.ssh.as_ref() {
        paths.push(&ssh.identity_file);
    }
    if let Some(path) = config.logging.path.as_deref() {
        paths.push(path);
    }
    if let Some(path) = config.tailscale.preauth_key_file.as_deref() {
        paths.push(path);
    }
    if let Some(libvirt) = config.runtime.libvirt() {
        paths.push(&libvirt.image_manifest_dir);
    }
    if let Some(path) = paths.into_iter().find(|path| is_protected_home_path(path)) {
        bail!(
            "systemd ProtectHome hides required path {}; move it outside /home, /root, and /run/user",
            path.display()
        );
    }
    Ok(())
}

fn is_protected_home_path(path: &Path) -> bool {
    ["/home", "/root", "/run/user"]
        .into_iter()
        .any(|root| path.starts_with(root))
}
