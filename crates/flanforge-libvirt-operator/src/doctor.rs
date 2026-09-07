use std::{os::unix::fs::PermissionsExt, path::Path};

use flanforge_core::{Config, GuestChannelKind};
use flanforge_runtime_libvirt::GuestChannelSupport;

use crate::{DoctorCheck, DoctorReport, OperatorError, image::ensure_config};

/// Reports standalone libvirt prerequisites without creating files or resources.
///
/// # Errors
/// Returns an error only when the effective configuration is invalid.
pub async fn doctor(config: &Config) -> Result<DoctorReport, OperatorError> {
    ensure_config(config)?;
    let backend = config
        .runtime
        .libvirt()
        .ok_or_else(|| OperatorError::Configuration {
            message: "doctor requires the libvirt backend".to_owned(),
        })?;
    let mut checks = Vec::new();
    checks.push(private_directory(&config.runtime.state_dir));
    checks.push(guest_identity(config).await);
    checks.push(guest_agent_contract(config).await);
    for (name, path) in [
        ("qemu-img executable", backend.qemu_img_path.as_path()),
        ("virsh executable", backend.virsh_path.as_path()),
    ] {
        checks.push(private_file(name, path, true));
    }
    checks.push(ssh_executable(config));
    match flanforge_runtime_libvirt::probe_operator(config).await {
        Ok(probe) => {
            let detail = format!(
                "helper can connect; {} machine(s) visible",
                probe.machine_count()
            );
            for name in [
                "libvirt connectivity",
                "storage pool",
                "virtual network",
                "storage headroom",
                "service permissions",
            ] {
                checks.push(DoctorCheck::passed(name, detail.clone()));
            }
            checks.push(DoctorCheck::passed(
                "immutable bases",
                format!("{} configured base(s) visible", probe.bases().len()),
            ));
        }
        Err(error) => {
            let detail = error.to_string();
            for name in [
                "libvirt connectivity",
                "storage pool",
                "virtual network",
                "storage headroom",
                "service permissions",
                "immutable bases",
            ] {
                checks.push(DoctorCheck::failed(name, detail.clone()));
            }
        }
    }
    Ok(DoctorReport::new(checks))
}

const GUEST_IDENTITY: &str = "guest SSH identity";

async fn guest_identity(config: &Config) -> DoctorCheck {
    match flanforge_runtime_libvirt::probe_guest_identity(config).await {
        Ok(true) => DoctorCheck::passed(GUEST_IDENTITY, "private key is usable"),
        Ok(false) => DoctorCheck::skipped(
            GUEST_IDENTITY,
            "no [guest.ssh] table is configured, so no key is seeded",
        ),
        Err(error) => DoctorCheck::failed(GUEST_IDENTITY, error.to_string()),
    }
}

/// `ssh` still carries a `qemu+ssh` libvirt transport whatever the guest
/// channel is, so it is scoped to the two uses rather than to the channel.
fn ssh_executable(config: &Config) -> DoctorCheck {
    const NAME: &str = "SSH executable";
    let is_needed = config.guest.channel == GuestChannelKind::Ssh
        || config
            .runtime
            .libvirt()
            .is_some_and(|libvirt| libvirt.uri.starts_with("qemu+ssh://"));
    if is_needed {
        private_file(NAME, config.runtime.ssh_path.as_path(), true)
    } else {
        DoctorCheck::skipped(
            NAME,
            "the agent channel and this libvirt transport both run without ssh",
        )
    }
}

/// The pre-boot gate fires at the first allocation, which is too late to be the
/// only warning: this reads the same published record it does.
async fn guest_agent_contract(config: &Config) -> DoctorCheck {
    const NAME: &str = "guest agent contract";
    if config.guest.channel != GuestChannelKind::Agent {
        return DoctorCheck::skipped(NAME, "the guest is driven over SSH");
    }
    let mut unknown = Vec::new();
    for (profile, support) in flanforge_runtime_libvirt::probe_guest_channel(config).await {
        match support {
            GuestChannelSupport::Supported => {}
            GuestChannelSupport::Unsupported(cause) => {
                return DoctorCheck::failed(NAME, format!("profile {profile}: {cause}"));
            }
            GuestChannelSupport::Unknown(cause) => unknown.push(format!("{profile}: {cause}")),
        }
    }
    if unknown.is_empty() {
        DoctorCheck::passed(NAME, "every profile base bakes the agent contract")
    } else {
        DoctorCheck::skipped(NAME, unknown.join("; "))
    }
}

fn private_directory(path: &Path) -> DoctorCheck {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_dir()
                && metadata.permissions().mode().trailing_zeros() >= 6 =>
        {
            DoctorCheck::passed("state directory", "private directory is accessible")
        }
        Ok(_) => DoctorCheck::failed("state directory", "path is not a private directory"),
        Err(error) => DoctorCheck::failed("state directory", error.to_string()),
    }
}

fn private_file(name: &'static str, path: &Path, executable: bool) -> DoctorCheck {
    // Tool paths follow symlinks: distributions routinely ship binaries as
    // links, and the daemon execs through them the same way.
    let metadata = if executable {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    match metadata {
        Ok(metadata)
            if metadata.file_type().is_file()
                && (!executable || metadata.permissions().mode() & 0o111 != 0) =>
        {
            DoctorCheck::passed(name, "bounded file mode is usable")
        }
        Ok(_) => DoctorCheck::failed(name, "path is not a usable regular file"),
        Err(error) => DoctorCheck::failed(name, error.to_string()),
    }
}
