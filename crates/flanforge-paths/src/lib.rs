mod defaults;
mod error;
mod home;
mod validate;

pub const CONFIG_PATH_ENV: &str = "FLANFORGE_CONFIG";

pub use defaults::{
    LaunchdPaths, LibvirtStatePaths, Platform, SystemdPaths, default_config_path,
    default_config_path_for, default_libvirt_image_manifest_dir, default_qemu_img_path,
    default_runner_host_path, default_runner_host_path_for, default_scp_path, default_ssh_path,
    default_tart_path, default_virsh_path, launchd_paths, libvirt_state_paths, linux_self_exe_path,
    warm_record_dir,
};
pub use error::PathError;
pub use home::{expand_home, service_home};
pub use validate::{ensure_absolute_normalized, is_absolute_normalized};

#[cfg(test)]
mod tests;
