use std::path::{Path, PathBuf};

use crate::{
    PathError, Platform, SystemdPaths, default_config_path_for, default_runner_host_path_for,
    expand_home, is_absolute_normalized, launchd_paths, libvirt_state_paths, linux_self_exe_path,
    warm_record_dir,
};

#[test]
fn platform_config_defaults_are_conventional() {
    assert_eq!(
        default_config_path_for(Platform::Linux, None)
            .unwrap_or_else(|error| unreachable!("linux config: {error}")),
        PathBuf::from("/etc/flanforge/config.toml")
    );
    assert_eq!(
        default_config_path_for(Platform::Macos, Some(Path::new("/Users/service")))
            .unwrap_or_else(|error| unreachable!("macOS config: {error}")),
        PathBuf::from("/Users/service/Library/Application Support/flanforge/config.toml")
    );
    assert_eq!(
        default_config_path_for(Platform::Macos, None),
        Err(PathError::HomeUnavailable)
    );
}

#[test]
fn linux_helper_executes_the_running_inode_not_the_installed_path() {
    assert_eq!(linux_self_exe_path(), PathBuf::from("/proc/self/exe"));
}

#[test]
fn provider_defaults_share_one_owner() {
    let launchd = launchd_paths(Path::new("/Users/service"), "dev.flanforge.flanforged")
        .unwrap_or_else(|error| unreachable!("launchd: {error}"));
    assert_eq!(
        launchd.binary,
        PathBuf::from("/Users/service/Library/Application Support/flanforge/bin/flanforged")
    );
    assert_eq!(
        launchd.definition,
        PathBuf::from("/Users/service/Library/LaunchAgents/dev.flanforge.flanforged.plist")
    );

    let systemd = SystemdPaths::default();
    assert_eq!(systemd.binary, PathBuf::from("/usr/local/bin/flanforged"));
    assert_eq!(
        systemd.environment,
        PathBuf::from("/etc/flanforge/environment")
    );
}

#[test]
fn runner_defaults_are_target_native() {
    assert_eq!(
        default_runner_host_path_for(Platform::Macos, Some(Path::new("/Users/runner")))
            .unwrap_or_else(|error| unreachable!("path: {error}")),
        PathBuf::from("/Users/runner/Library/Application Support/flanforge/bin/forgejo-runner")
    );
    assert_eq!(
        default_runner_host_path_for(Platform::Linux, None)
            .unwrap_or_else(|error| unreachable!("path: {error}")),
        PathBuf::from("/usr/local/libexec/flanforge/forgejo-runner")
    );
}

#[test]
fn home_expansion_is_explicit_and_validated() {
    assert_eq!(
        expand_home(Path::new("~/state"), Some(Path::new("/srv/flanforge")))
            .unwrap_or_else(|error| unreachable!("expand: {error}")),
        PathBuf::from("/srv/flanforge/state")
    );
    assert_eq!(
        expand_home(Path::new("~/state"), Some(Path::new("relative"))),
        Err(PathError::HomeNotAbsolute)
    );
    assert_eq!(
        expand_home(Path::new("~/state"), None),
        Err(PathError::HomeUnavailable)
    );
}

#[test]
fn normalized_paths_reject_relative_and_traversal_components() {
    assert!(is_absolute_normalized(Path::new("/var/lib/flanforge")));
    for rejected in [
        "var/lib/flanforge",
        "/var/../lib/flanforge",
        "/var/./lib/flanforge",
        "/var//lib/flanforge",
    ] {
        assert!(
            !is_absolute_normalized(Path::new(rejected)),
            "accepted {rejected}"
        );
    }
}

#[test]
fn libvirt_and_warm_state_namespaces_are_distinct_and_derived() {
    let state = Path::new("/srv/flanforge");
    let libvirt = libvirt_state_paths(state);
    assert_eq!(
        libvirt.published_bases,
        PathBuf::from("/srv/flanforge/libvirt/published-bases")
    );
    assert_eq!(
        warm_record_dir(state),
        PathBuf::from("/srv/flanforge/images")
    );
    assert_ne!(libvirt.published_bases, warm_record_dir(state));
}
