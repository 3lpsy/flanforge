use std::path::Path;

use flanforge_test_support::config;

use crate::{
    constants::NOT_LOADED_EXIT_CODE,
    control::{ensure_started, is_not_loaded_exit, last_exit_code},
    install::launch_agent_plist,
    paths::LaunchdPaths,
};

#[test]
fn launch_agent_preserves_arguments_and_covers_shutdown_grace() {
    let paths = LaunchdPaths {
        binary_dir: "/Users/a&b/bin".into(),
        binary: "/Users/a&b/bin/flanforged".into(),
        definition: "/Users/a/agent.plist".into(),
        log_dir: "/Users/a/log".into(),
        stdout_log: "/Users/a/log/out".into(),
        stderr_log: "/Users/a/log/error".into(),
    };
    let plist = launch_agent_plist(&paths, Path::new("/Users/a&b/config.toml"), 30)
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert!(plist.contains("/Users/a&amp;b/bin/flanforged"));
    assert!(plist.contains("<string>daemon</string>"));
    assert!(plist.contains("<key>ExitTimeOut</key><integer>35</integer>"));
}

#[test]
fn launchctl_only_classifies_the_missing_service_exit_as_not_loaded() {
    assert!(is_not_loaded_exit(Some(NOT_LOADED_EXIT_CODE)));
    assert!(!is_not_loaded_exit(Some(1)));
    assert!(!is_not_loaded_exit(None));
}

#[test]
fn launchctl_status_parser_reads_exit_code() {
    assert_eq!(last_exit_code("last exit code = 7\n"), Some(7));
    assert_eq!(last_exit_code("state = running\n"), None);
}

#[test]
fn launchctl_provider_accepts_tart_configuration_shape() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
    let config = config(directory.path().join("state"));
    assert_eq!(
        config.runtime.backend_kind(),
        flanforge_core::RuntimeBackendKind::Tart
    );
}

/// RUN-283: launchd reports the *previous* run's exit code under `KeepAlive`,
/// so a stop during a job -- which exits non-zero by design -- must not make
/// the next start report a failure about a healthy daemon.
#[test]
fn a_start_ignores_an_exit_code_its_own_run_did_not_produce() {
    let running_after_a_failed_stop = "state = running\nlast exit code = 3\n";

    assert!(
        ensure_started(running_after_a_failed_stop, Some(3)).is_ok(),
        "an unchanged exit code describes the run that already ended"
    );

    // The same report with no prior sample is a code this start produced.
    let Err(error) = ensure_started(running_after_a_failed_stop, None) else {
        unreachable!("a newly appeared exit code is this start's");
    };
    assert!(
        error.to_string().contains("restarted after exiting"),
        "{error}"
    );

    // Died and stayed down: the code is the reason, and it is reported.
    let Err(error) = ensure_started("state = not running\nlast exit code = 3\n", Some(0)) else {
        unreachable!("a service that is not running is a failure");
    };
    assert!(error.to_string().contains("status 3"), "{error}");

    // Down with nothing to explain it.
    let Err(error) = ensure_started("state = not running\n", None) else {
        unreachable!("a service that is not running is a failure");
    };
    assert!(error.to_string().contains("running state"), "{error}");

    // A clean previous run leaves nothing to report either way.
    assert!(ensure_started("state = running\nlast exit code = 0\n", None).is_ok());
}
