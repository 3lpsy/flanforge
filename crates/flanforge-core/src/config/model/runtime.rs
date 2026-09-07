use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::VmPrefix;

use super::{
    LibvirtConfig, RuntimeBackendConfig, RuntimeBackendKind, TartConfig,
    backend::{default_runner_host_path, default_scp_path, default_ssh_path, default_tart_path},
};

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub state_dir: PathBuf,
    #[serde(default)]
    pub backend: RuntimeBackendConfig,
    #[serde(default = "default_ssh_path")]
    pub ssh_path: PathBuf,
    #[serde(default = "default_scp_path")]
    pub scp_path: PathBuf,
    pub vm_prefix: VmPrefix,
    #[serde(default = "default_max_running_vms")]
    pub max_running_vms: u8,
    /// Host slots the hot pool may hold, capped by `max_running_vms`. Unlike
    /// `max_running_vms` the default does not vary by backend: zero is zero,
    /// and zero is the documented global kill switch.
    #[serde(default)]
    pub max_hot_vms: u8,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    #[serde(default = "default_reap_interval_hours")]
    pub reap_interval_hours: u64,
    #[serde(default)]
    pub host_cpu_count: Option<u8>,
    #[serde(default)]
    pub host_memory_mb: Option<u32>,
    #[serde(default)]
    pub host_storage_mb: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfigDocument {
    state_dir: PathBuf,
    backend: Option<RuntimeBackendConfig>,
    #[serde(default)]
    tart_path: Option<PathBuf>,
    #[serde(default = "default_ssh_path")]
    ssh_path: PathBuf,
    #[serde(default = "default_scp_path")]
    scp_path: PathBuf,
    #[serde(default)]
    forgejo_runner_host_path: Option<PathBuf>,
    vm_prefix: VmPrefix,
    #[serde(default)]
    tart_home: Option<PathBuf>,
    max_running_vms: Option<u8>,
    #[serde(default)]
    max_hot_vms: Option<u8>,
    #[serde(default = "default_poll_seconds")]
    poll_seconds: u64,
    #[serde(default = "default_reap_interval_hours")]
    reap_interval_hours: u64,
    #[serde(default)]
    host_cpu_count: Option<u8>,
    #[serde(default)]
    host_memory_mb: Option<u32>,
    #[serde(default)]
    host_storage_mb: Option<u64>,
}

impl<'de> Deserialize<'de> for RuntimeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let document = RuntimeConfigDocument::deserialize(deserializer)?;
        let has_legacy_tart = document.tart_path.is_some()
            || document.forgejo_runner_host_path.is_some()
            || document.tart_home.is_some();
        if document.backend.is_some() && has_legacy_tart {
            return Err(serde::de::Error::custom(
                "legacy Tart fields cannot be combined with runtime.backend",
            ));
        }

        let mut backend = document.backend.unwrap_or_else(|| {
            if has_legacy_tart {
                RuntimeBackendConfig::Tart(TartConfig {
                    path: document.tart_path.unwrap_or_else(default_tart_path),
                    home: document.tart_home,
                    // Legacy flat configs predate image-baked runners, so
                    // their omission keeps the old host-copy default.
                    runner_host_path: Some(
                        document
                            .forgejo_runner_host_path
                            .unwrap_or_else(default_runner_host_path),
                    ),
                })
            } else {
                RuntimeBackendConfig::default()
            }
        });
        if let RuntimeBackendConfig::Libvirt(libvirt) = &mut backend
            && libvirt.image_manifest_dir.as_os_str().is_empty()
        {
            libvirt.image_manifest_dir =
                flanforge_paths::libvirt_state_paths(&document.state_dir).published_bases;
        }
        let max_running_vms = document.max_running_vms.unwrap_or(match &backend {
            RuntimeBackendConfig::Tart(_) => default_max_running_vms(),
            RuntimeBackendConfig::Libvirt(_) => 1,
        });

        Ok(Self {
            state_dir: document.state_dir,
            backend,
            ssh_path: document.ssh_path,
            scp_path: document.scp_path,
            vm_prefix: document.vm_prefix,
            max_running_vms,
            max_hot_vms: document.max_hot_vms.unwrap_or(0),
            poll_seconds: document.poll_seconds,
            reap_interval_hours: document.reap_interval_hours,
            host_cpu_count: document.host_cpu_count,
            host_memory_mb: document.host_memory_mb,
            host_storage_mb: document.host_storage_mb,
        })
    }
}

impl RuntimeConfig {
    #[must_use]
    pub const fn backend_kind(&self) -> RuntimeBackendKind {
        self.backend.kind()
    }

    #[must_use]
    pub const fn libvirt(&self) -> Option<&LibvirtConfig> {
        match &self.backend {
            RuntimeBackendConfig::Tart(_) => None,
            RuntimeBackendConfig::Libvirt(config) => Some(config),
        }
    }

    #[must_use]
    pub const fn libvirt_mut(&mut self) -> Option<&mut LibvirtConfig> {
        match &mut self.backend {
            RuntimeBackendConfig::Tart(_) => None,
            RuntimeBackendConfig::Libvirt(config) => Some(config),
        }
    }

    #[must_use]
    pub const fn tart(&self) -> Option<&TartConfig> {
        match &self.backend {
            RuntimeBackendConfig::Tart(config) => Some(config),
            RuntimeBackendConfig::Libvirt(_) => None,
        }
    }

    #[must_use]
    pub const fn tart_mut(&mut self) -> Option<&mut TartConfig> {
        match &mut self.backend {
            RuntimeBackendConfig::Tart(config) => Some(config),
            RuntimeBackendConfig::Libvirt(_) => None,
        }
    }
}

const fn default_max_running_vms() -> u8 {
    2
}

const fn default_poll_seconds() -> u64 {
    2
}

const fn default_reap_interval_hours() -> u64 {
    168
}
