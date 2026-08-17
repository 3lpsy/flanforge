use std::path::Path;

use flanforge_core::GuestConfig;

/// Builds the daemon-owned SSH/SCP options. `-F /dev/null` drops the operator's
/// `ssh_config`, and the named options restate what it must never inherit.
pub(super) fn base_arguments(config: &GuestConfig) -> Vec<String> {
    let mut arguments = vec![
        "-F".into(),
        "/dev/null".into(),
        "-o".into(),
        "GlobalKnownHostsFile=/dev/null".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "ForwardAgent=no".into(),
        "-o".into(),
        "ForwardX11=no".into(),
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        "ControlPath=none".into(),
        "-o".into(),
        "PermitLocalCommand=no".into(),
        "-o".into(),
        "ProxyCommand=none".into(),
    ];
    arguments.extend(host_key_arguments(config));
    arguments.extend([
        "-o".into(),
        format!("ConnectTimeout={}", config.ssh_connect_timeout_seconds),
        "-i".into(),
        config.ssh_identity_file.display().to_string(),
    ]);
    arguments
}

/// Only host-key verification changes with `verify_host_key`; every other
/// hardening option stays in force. The pinning inputs are absent from the
/// disabled vector, and unset inputs fail closed against an empty anchor.
fn host_key_arguments(config: &GuestConfig) -> Vec<String> {
    if !config.verify_host_key {
        return vec![
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
        ];
    }
    let known_hosts = config
        .ssh_known_hosts_file
        .as_deref()
        .unwrap_or_else(|| Path::new("/dev/null"));
    let mut arguments = vec![
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        format!("UserKnownHostsFile=\"{}\"", known_hosts.display()),
    ];
    if let Some(alias) = &config.ssh_host_key_alias {
        arguments.extend(["-o".into(), format!("HostKeyAlias={alias}")]);
    }
    arguments
}
