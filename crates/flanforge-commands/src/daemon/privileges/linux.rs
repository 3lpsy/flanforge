use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_config::{ConfigOverrides, load_config_with_overrides};
use flanforge_core::RuntimeBackendConfig;
use flanforge_service_systemd::{PathAccess, is_user_path_accessible, run_as_user};

use flanforge_cli::DaemonPrivArgs;

use crate::daemon::service;

pub(super) async fn priv_gates(
    config_path: &Path,
    arguments: DaemonPrivArgs,
    overrides: &ConfigOverrides,
) -> Result<()> {
    ensure_report_only(&arguments)?;
    let config = load_config_with_overrides(config_path, overrides)
        .await
        .context("cannot load service configuration")?;
    config
        .ensure_valid()
        .context("invalid service configuration")?;
    let RuntimeBackendConfig::Libvirt(libvirt) = &config.runtime.backend else {
        bail!("Linux privilege checks require the libvirt runtime backend");
    };
    let status = service::native().status().await?;
    if !status.is_installed {
        bail!("systemd service is not installed; run `flanforged daemon install`");
    }
    let user = status.user.as_deref().unwrap_or("root");
    println!("user:       {user}");
    // Printed beside the user so a skipped guest-key check below is read as
    // inapplicable rather than as one that passed.
    println!("channel:    {}", config.guest.channel);
    check_path(user, "config", config_path, &[PathAccess::Read]).await?;
    check_path(
        user,
        "state",
        &config.runtime.state_dir,
        &[PathAccess::Write, PathAccess::Search],
    )
    .await?;
    if let Some(ssh) = config.guest.ssh.as_ref() {
        check_path(user, "guest key", &ssh.identity_file, &[PathAccess::Read]).await?;
    }
    check_path(
        user,
        "Forgejo token",
        &config.forgejo.api_token_file,
        &[PathAccess::Read],
    )
    .await?;
    check_path(
        user,
        "image manifests",
        &libvirt.image_manifest_dir,
        &[PathAccess::Read, PathAccess::Search],
    )
    .await?;
    check_virsh(
        user,
        &libvirt.virsh_path,
        &["--connect", &libvirt.uri, "uri"],
        "connection",
    )
    .await?;
    check_virsh(
        user,
        &libvirt.virsh_path,
        &["--connect", &libvirt.uri, "pool-info", &libvirt.pool],
        "storage pool",
    )
    .await?;
    check_virsh(
        user,
        &libvirt.virsh_path,
        &["--connect", &libvirt.uri, "net-info", &libvirt.network],
        "network",
    )
    .await?;
    println!(
        "libvirt:    {}, pool, and network are accessible",
        libvirt.uri
    );
    Ok(())
}

fn ensure_report_only(arguments: &DaemonPrivArgs) -> Result<()> {
    if arguments.grant_firewall
        || arguments.prompt
        || arguments.elevated.is_some()
        || arguments.probe
    {
        bail!(
            "Linux `daemon priv` is report-only; grant the installed unit user narrow libvirt ACL and path access through host provisioning"
        );
    }
    Ok(())
}

async fn check_path(user: &str, label: &str, path: &Path, modes: &[PathAccess]) -> Result<()> {
    for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
        if !is_user_path_accessible(user, ancestor, PathAccess::Search).await? {
            bail!(
                "installed unit user {user} cannot search the {label} ancestor {}",
                ancestor.display()
            );
        }
    }
    for mode in modes {
        if !is_user_path_accessible(user, path, *mode).await? {
            bail!(
                "installed unit user {user} cannot access {label} at {}",
                path.display()
            );
        }
    }
    println!("{label}: {}", path.display());
    Ok(())
}

async fn check_virsh(user: &str, virsh_path: &Path, arguments: &[&str], label: &str) -> Result<()> {
    let status = run_as_user(user, virsh_path, arguments).await?;
    if status.success() {
        Ok(())
    } else {
        bail!("installed unit user {user} cannot access the configured libvirt {label}")
    }
}
