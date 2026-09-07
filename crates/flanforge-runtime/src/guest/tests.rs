use super::input::GuestRunnerInput;
use super::marker::retention_marker_script;
use super::paths::{temporary_guest_paths, temporary_guest_patterns};
use super::script::runner_script;
use super::tailscale::{LIBVIRT_TAILSCALE_PATH, TART_TAILSCALE_PATH, path};
use flanforge_core::RuntimeBackendKind;
use validator::Validate;

#[test]
fn legacy_temporary_path_metadata_is_not_a_retention_command() {
    let marker = retention_marker_script();
    assert_eq!(temporary_guest_paths().len(), 3);
    assert_eq!(temporary_guest_patterns().len(), 3);
    for value in temporary_guest_paths()
        .iter()
        .chain(temporary_guest_patterns())
    {
        assert!(!marker.contains(value), "{value}: {marker}");
    }
}

#[test]
fn tailscale_binary_is_selected_by_guest_platform() {
    assert_eq!(path(RuntimeBackendKind::Tart), TART_TAILSCALE_PATH);
    assert_eq!(path(RuntimeBackendKind::Libvirt), LIBVIRT_TAILSCALE_PATH);
}

/// CORE-321: Forgejo publishes the job handle as opaque, so any bounded
/// shell-safe token is accepted while the runner registration UUID stays one.
#[test]
fn an_opaque_job_handle_is_accepted_but_an_unbounded_or_hostile_one_is_not() {
    let input = |handle: &str| GuestRunnerInput {
        runner_path: "/usr/local/bin/forgejo-runner".into(),
        server_url: "https://git.example".into(),
        uuid: "392c9434-6bb9-454b-b9ff-646875cf6691".into(),
        label: "macos-allocation:host".into(),
        handle: handle.to_owned(),
    };
    let longest = "a".repeat(128);
    for handle in [
        "job_01HZX3QK",
        "33ba7d51-59c6-44f8-9d2b-1b94f4033973",
        "forgejo:run.42-1",
        longest.as_str(),
    ] {
        assert!(input(handle).validate().is_ok(), "{handle}");
    }

    let too_long = "a".repeat(129);
    for handle in [
        "",
        too_long.as_str(),
        "$(touch /tmp/host)",
        "job'01HZX3QK",
        "job 01HZX3QK",
        "job;touch",
        "job\n01HZX3QK",
    ] {
        assert!(input(handle).validate().is_err(), "{handle:?}");
    }

    // The runner registration UUID is a real UUID upstream and stays checked.
    let mut opaque_uuid = input("job_01HZX3QK");
    opaque_uuid.uuid = "runner_01HZX3QK".into();
    assert!(opaque_uuid.validate().is_err());
}

/// The bounded charset replaces the UUID check only because every interpolated
/// value reaches the guest as a single-quoted shell word (CORE-321).
#[test]
fn the_runner_command_single_quotes_the_job_handle() {
    let script = runner_script(
        "/usr/local/bin/forgejo-runner",
        "https://git.example",
        "392c9434-6bb9-454b-b9ff-646875cf6691",
        "macos-allocation:host",
        "job_01HZX3QK",
    );
    assert!(script.contains("--handle 'job_01HZX3QK'"), "{script}");
}
