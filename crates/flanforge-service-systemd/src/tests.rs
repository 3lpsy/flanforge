use std::path::Path;

use flanforge_service::LogOptions;
use flanforge_test_support::config;

use crate::{
    constants::{COMMAND_TIMEOUT, DEFAULT_USER, GETENT_NOT_FOUND_EXIT_CODE, JOURNALCTL_PATH},
    control::{journalctl_command, journalctl_timeout, property},
    install::{is_getent_user_missing, owner_spec, systemd_unit},
    user::ensure_user_name,
};

#[test]
fn systemd_unit_uses_safe_defaults_and_no_libvirt_topology() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let mut config = (*config(directory.path().join("state"))).clone();
    config.server.shutdown_grace_seconds = 30;
    let paths = flanforge_paths::SystemdPaths::default();
    let unit = systemd_unit(&paths, Path::new("/etc/flanforge/config.toml"), &config)
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(unit.contains(&format!("User={DEFAULT_USER}")));
    assert!(unit.contains("daemon run"));
    assert!(unit.contains("TimeoutStopSec=35s"));
    assert!(unit.contains("NoNewPrivileges=true"));
    assert!(unit.contains("PrivateDevices=true"));
    assert!(unit.contains("UMask=0077"));
    assert!(unit.contains("StateDirectory=flanforge"));
    assert!(unit.contains("RuntimeDirectory=flanforge"));
    assert!(!unit.contains("virtqemud"));
    assert!(!unit.contains("virtnetworkd"));
    assert!(!unit.contains("virtstoraged"));
    assert!(!unit.contains("libvirtd"));
    assert!(!unit.contains("SupplementaryGroups="));

    // The default db lives beside the config, so its directory must be
    // writable under ProtectSystem=strict; an explicit path in the state dir
    // collapses back to one entry.
    let state_dir = directory.path().join("state");
    assert!(unit.contains(&format!(
        "ReadWritePaths=\"{}\" \"/etc/flanforge\"\n",
        state_dir.display()
    )));
    config.db.db_path = Some(state_dir.join("flanforge.db"));
    let unit = systemd_unit(&paths, Path::new("/etc/flanforge/config.toml"), &config)
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(unit.contains(&format!("ReadWritePaths=\"{}\"\n", state_dir.display())));
}

#[test]
fn journal_reads_are_bounded_unless_following() {
    let bounded = LogOptions::new(false, 50, false);
    let command = journalctl_command(bounded);
    assert_eq!(command.as_std().get_program(), JOURNALCTL_PATH);
    assert_eq!(
        command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["--unit", "flanforged.service", "--lines", "50"]
    );
    assert_eq!(journalctl_timeout(bounded), Some(COMMAND_TIMEOUT));

    let following = LogOptions::new(true, 25, false);
    assert!(
        journalctl_command(following)
            .as_std()
            .get_args()
            .any(|argument| argument == "--follow")
    );
    assert_eq!(journalctl_timeout(following), None);
}

#[test]
fn systemd_unit_rejects_paths_hidden_by_protect_home() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let mut config = (*config(directory.path().join("state"))).clone();
    let paths = flanforge_paths::SystemdPaths::default();

    config.forgejo.api_token_file = "/home/runner/flanforge/token".into();
    assert!(systemd_unit(&paths, Path::new("/etc/flanforge/config.toml"), &config).is_err());

    config.forgejo.api_token_file = "/etc/flanforge/token".into();
    assert!(systemd_unit(&paths, Path::new("/root/flanforge.toml"), &config).is_err());
}

#[test]
fn systemd_unit_rejects_hostile_paths_and_escapes_specifiers() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let config = (*config(directory.path().join("state"))).clone();
    let paths = flanforge_paths::SystemdPaths::default();
    for rejected in [
        "relative/config.toml",
        "/etc/flanforge/../config.toml",
        "/etc/flanforge/bad\nunit.toml",
        "/etc/flanforge/\"bad.toml",
        "/etc/flanforge/bad\\unit.toml",
    ] {
        assert!(
            systemd_unit(&paths, Path::new(rejected), &config).is_err(),
            "accepted {rejected:?}"
        );
    }

    let unit = systemd_unit(&paths, Path::new("/etc/flanforge/100%-$name.toml"), &config)
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(unit.contains("100%%-$$name.toml"));
}

#[test]
fn systemd_status_parser_reads_effective_unit_user() {
    let report = "ActiveState=active\nSubState=running\nUser=builder\nMainPID=42\n";
    assert_eq!(property(report, "User"), Some("builder"));
    assert_eq!(property(report, "MainPID"), Some("42"));
    assert_eq!(owner_spec("builder"), "builder");
    assert!(!owner_spec("builder").contains(':'));
}

#[test]
fn only_getent_missing_user_status_allows_account_creation() {
    assert!(is_getent_user_missing(Some(GETENT_NOT_FOUND_EXIT_CODE)));
    assert!(!is_getent_user_missing(Some(1)));
    assert!(!is_getent_user_missing(Some(3)));
    assert!(!is_getent_user_missing(None));
}

#[test]
fn installed_unit_users_are_structurally_validated() {
    assert!(ensure_user_name("builder-1").is_ok());
    assert!(ensure_user_name("1001").is_ok());
    for rejected in ["", "-root", "bad user", "user:group", "user\nroot"] {
        assert!(ensure_user_name(rejected).is_err(), "accepted {rejected:?}");
    }
}
