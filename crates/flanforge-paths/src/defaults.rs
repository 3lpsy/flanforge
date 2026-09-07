use std::path::{Path, PathBuf};

use crate::{PathError, service_home};

const MACOS_SUPPORT: &str = "Library/Application Support/flanforge";
const MACOS_LOGS: &str = "Library/Logs/flanforged";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Macos,
    Linux,
}

impl Platform {
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Macos;
        #[cfg(target_os = "linux")]
        return Self::Linux;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        compile_error!("FlanForge supports only macOS and Linux hosts");
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchdPaths {
    pub binary_dir: PathBuf,
    pub binary: PathBuf,
    pub definition: PathBuf,
    pub log_dir: PathBuf,
    pub stdout_log: PathBuf,
    pub stderr_log: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemdPaths {
    pub binary: PathBuf,
    pub definition: PathBuf,
    pub environment: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibvirtStatePaths {
    pub root: PathBuf,
    pub service_instance: PathBuf,
    pub allocations: PathBuf,
    pub imports: PathBuf,
    pub published_bases: PathBuf,
    /// Warm pointer documents and warm capture intents.
    pub warm: PathBuf,
}

impl Default for SystemdPaths {
    fn default() -> Self {
        Self {
            binary: "/usr/local/bin/flanforged".into(),
            definition: "/etc/systemd/system/flanforged.service".into(),
            environment: "/etc/flanforge/environment".into(),
            state_dir: "/var/lib/flanforge".into(),
            runtime_dir: "/run/flanforge".into(),
        }
    }
}

/// Derives launchd locations from an explicit home and service label.
///
/// # Errors
/// Returns an error when `home` or `service_label` is unsafe.
pub fn launchd_paths(home: &Path, service_label: &str) -> Result<LaunchdPaths, PathError> {
    if !home.is_absolute() {
        return Err(PathError::HomeNotAbsolute);
    }
    if !(1..=128).contains(&service_label.len())
        || !service_label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        || service_label.contains("..")
        || !service_label
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !service_label
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(PathError::UnsafeName);
    }
    let support = home.join(MACOS_SUPPORT);
    let binary_dir = support.join("bin");
    let log_dir = home.join(MACOS_LOGS);
    Ok(LaunchdPaths {
        binary: binary_dir.join("flanforged"),
        binary_dir,
        definition: home
            .join("Library/LaunchAgents")
            .join(format!("{service_label}.plist")),
        stdout_log: log_dir.join("stdout.log"),
        stderr_log: log_dir.join("stderr.log"),
        log_dir,
    })
}

/// Returns the current target's conventional configuration path.
///
/// # Errors
/// Returns an error when macOS has no valid service home.
pub fn default_config_path() -> Result<PathBuf, PathError> {
    let platform = Platform::current();
    let home = matches!(platform, Platform::Macos)
        .then(service_home)
        .transpose()?;
    default_config_path_for(platform, home.as_deref())
}

/// Returns a platform's configuration path without target detection.
///
/// # Errors
/// Returns an error when a macOS home is absent or not absolute.
pub fn default_config_path_for(
    platform: Platform,
    home: Option<&Path>,
) -> Result<PathBuf, PathError> {
    match platform {
        Platform::Macos => {
            let home = home.ok_or(PathError::HomeUnavailable)?;
            if !home.is_absolute() {
                return Err(PathError::HomeNotAbsolute);
            }
            Ok(home.join(MACOS_SUPPORT).join("config.toml"))
        }
        Platform::Linux => Ok("/etc/flanforge/config.toml".into()),
    }
}

#[must_use]
pub fn default_ssh_path() -> PathBuf {
    "/usr/bin/ssh".into()
}

#[must_use]
pub fn default_scp_path() -> PathBuf {
    "/usr/bin/scp".into()
}

#[must_use]
pub fn default_tart_path() -> PathBuf {
    "/opt/homebrew/bin/tart".into()
}

#[must_use]
pub fn default_runner_host_path() -> PathBuf {
    match Platform::current() {
        Platform::Macos => "~/Library/Application Support/flanforge/bin/forgejo-runner".into(),
        Platform::Linux => "/usr/local/libexec/flanforge/forgejo-runner".into(),
    }
}

/// Returns the absolute target-native runner path for a known platform.
///
/// # Errors
/// Returns an error when macOS has no valid service home.
pub fn default_runner_host_path_for(
    platform: Platform,
    home: Option<&Path>,
) -> Result<PathBuf, PathError> {
    match platform {
        Platform::Macos => {
            let home = home.ok_or(PathError::HomeUnavailable)?;
            if !home.is_absolute() {
                return Err(PathError::HomeNotAbsolute);
            }
            Ok(home.join(MACOS_SUPPORT).join("bin/forgejo-runner"))
        }
        Platform::Linux => Ok("/usr/local/libexec/flanforge/forgejo-runner".into()),
    }
}

#[must_use]
pub fn default_qemu_img_path() -> PathBuf {
    "/usr/bin/qemu-img".into()
}

#[must_use]
pub fn default_virsh_path() -> PathBuf {
    "/usr/bin/virsh".into()
}

/// Resolves to the inode of the currently running Linux executable, even when
/// its installed pathname is atomically replaced during an upgrade.
#[must_use]
pub fn linux_self_exe_path() -> PathBuf {
    "/proc/self/exe".into()
}

#[must_use]
pub fn default_libvirt_image_manifest_dir() -> PathBuf {
    libvirt_state_paths(&SystemdPaths::default().state_dir).published_bases
}

#[must_use]
pub fn libvirt_state_paths(state_dir: &Path) -> LibvirtStatePaths {
    let root = state_dir.join("libvirt");
    LibvirtStatePaths {
        service_instance: root.join("service-instance-id"),
        allocations: root.join("allocations"),
        imports: root.join("imports"),
        published_bases: root.join("published-bases"),
        warm: root.join("warm"),
        root,
    }
}

#[must_use]
pub fn warm_record_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("images")
}
