use std::path::Path;

use super::{control::last_exit_code, install::launch_agent_plist, paths::ServicePaths};

#[test]
fn launch_agent_arguments_are_xml_escaped_and_use_daemon_run() {
    let paths = ServicePaths {
        binary_dir: "/Users/a&b/bin".into(),
        binary: "/Users/a&b/bin/flanforged".into(),
        launch_agent: "/Users/a/agent.plist".into(),
        log_dir: "/Users/a/log".into(),
        stdout_log: "/Users/a/log/out".into(),
        stderr_log: "/Users/a/log/error".into(),
    };
    let plist = launch_agent_plist(&paths, Path::new("/Users/a&b/config.toml"));
    assert!(plist.contains("/Users/a&amp;b/bin/flanforged"));
    assert!(plist.contains("/Users/a&amp;b/config.toml"));
    assert!(plist.contains("<string>daemon</string>"));
    assert!(plist.contains("<string>run</string>"));
    assert!(plist.contains("<key>ThrottleInterval</key><integer>30</integer>"));
}

#[test]
fn a_non_zero_last_exit_code_is_read_from_the_launchd_report() {
    assert_eq!(
        last_exit_code("\tstate = not running\n\tlast exit code = 1\n"),
        Some(1)
    );
    assert_eq!(last_exit_code("\tlast exit code = 0\n"), Some(0));
    assert_eq!(last_exit_code("\tstate = running\n"), None);
}
