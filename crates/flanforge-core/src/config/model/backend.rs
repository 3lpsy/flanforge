use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeBackendConfig {
    Tart(TartConfig),
    Libvirt(LibvirtConfig),
}

impl Default for RuntimeBackendConfig {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Tart(TartConfig::default());
        #[cfg(target_os = "linux")]
        return Self::Libvirt(LibvirtConfig::default());
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        compile_error!("FlanForge supports only macOS and Linux hosts");
    }
}

impl RuntimeBackendConfig {
    #[must_use]
    pub const fn kind(&self) -> RuntimeBackendKind {
        match self {
            Self::Tart(_) => RuntimeBackendKind::Tart,
            Self::Libvirt(_) => RuntimeBackendKind::Libvirt,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TartConfig {
    #[serde(default = "default_tart_path")]
    pub path: PathBuf,
    #[serde(default)]
    pub home: Option<PathBuf>,
    /// Set to copy the runner from this host path into every guest; unset
    /// uses the runner the base image bakes at `guest.forgejo_runner_path`.
    #[serde(default)]
    pub runner_host_path: Option<PathBuf>,
}

impl Default for TartConfig {
    fn default() -> Self {
        Self {
            path: default_tart_path(),
            home: None,
            runner_host_path: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Tart,
    Libvirt,
}

impl Default for RuntimeBackendKind {
    fn default() -> Self {
        RuntimeBackendConfig::default().kind()
    }
}

impl std::fmt::Display for RuntimeBackendKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tart => formatter.write_str("tart"),
            Self::Libvirt => formatter.write_str("libvirt"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LibvirtConfig {
    pub uri: String,
    pub pool: String,
    pub network: String,
    #[serde(default)]
    pub image_manifest_dir: PathBuf,
    #[serde(default = "flanforge_paths::default_qemu_img_path")]
    pub qemu_img_path: PathBuf,
    #[serde(default = "flanforge_paths::default_virsh_path")]
    pub virsh_path: PathBuf,
    #[serde(default = "default_min_storage_free_mb")]
    pub min_storage_free_mb: u64,
    /// Budget for one warm capture. `cleanup_timeout_seconds` caps at 600s,
    /// which is a clone budget rather than a qcow2-convert budget.
    #[serde(default = "default_warm_capture_timeout_seconds")]
    pub warm_capture_timeout_seconds: u64,
    /// Admits `qemu+tcp`, which carries libvirt's root-equivalent API in clear
    /// text. Leave false unless the transport is already on a trusted link.
    #[serde(default)]
    pub allow_insecure_transport: bool,
    /// Scanned at startup for `<name>.qcow2` + `<name>.manifest.json` pairs to
    /// import automatically. Unset disables the scan.
    #[serde(default)]
    pub image_import_dir: Option<PathBuf>,
    /// What the startup scan does with a published name whose content changed.
    #[serde(default)]
    pub image_reimport: ImageReimport,
}

/// `refuse` keeps publications immutable; `supersede` republishes the record
/// and leaves the superseded volume for the operator or the sweep.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageReimport {
    #[default]
    Refuse,
    Supersede,
}

impl LibvirtConfig {
    /// Whether the transport carries libvirt's root-equivalent API without
    /// authentication or encryption. The guest agent channel is neither
    /// defaulted to nor accepted over it, because the runner registration
    /// token travels inside agent commands.
    #[must_use]
    pub fn is_clear_text_transport(&self) -> bool {
        self.uri.starts_with("qemu+tcp://")
    }
}

impl Default for LibvirtConfig {
    fn default() -> Self {
        Self {
            uri: LIBVIRT_SYSTEM_URI.to_owned(),
            pool: "flanforge".to_owned(),
            network: "flanforge-ci".to_owned(),
            image_manifest_dir: PathBuf::new(),
            qemu_img_path: flanforge_paths::default_qemu_img_path(),
            virsh_path: flanforge_paths::default_virsh_path(),
            min_storage_free_mb: default_min_storage_free_mb(),
            warm_capture_timeout_seconds: default_warm_capture_timeout_seconds(),
            allow_insecure_transport: false,
            image_import_dir: None,
            image_reimport: ImageReimport::default(),
        }
    }
}

pub const LIBVIRT_SYSTEM_URI: &str = "qemu:///system";
pub const LIBVIRT_GUEST_USER: &str = "runner";

pub(super) fn default_tart_path() -> PathBuf {
    flanforge_paths::default_tart_path()
}

pub(super) fn default_ssh_path() -> PathBuf {
    flanforge_paths::default_ssh_path()
}

pub(super) fn default_scp_path() -> PathBuf {
    flanforge_paths::default_scp_path()
}

pub(super) fn default_runner_host_path() -> PathBuf {
    flanforge_paths::default_runner_host_path()
}

const fn default_min_storage_free_mb() -> u64 {
    32_768
}

const fn default_warm_capture_timeout_seconds() -> u64 {
    1_800
}
