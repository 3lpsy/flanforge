use std::time::Duration;

use super::{GuestCapture, GuestCommand, GuestExit, GuestProgram, GuestSecret, GuestSpawn};

const TOKEN: &str = "09d130cf90f9d757d83e5cc5a5338c470f04b71c";

#[test]
fn a_secret_is_stored_as_the_line_the_guest_reads() {
    let secret = GuestSecret::line(TOKEN).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_eq!(secret.as_bytes(), format!("{TOKEN}\n").as_bytes());
}

/// The value must never reach a log, so `Debug` reports only how long it is.
#[test]
fn a_secret_debugs_as_a_length_and_never_as_its_bytes() {
    let secret = GuestSecret::line(TOKEN).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");
    assert!(rendered.contains("41"), "{rendered}");
}

#[test]
fn a_secret_is_bounded_and_printable() {
    let longest = "a".repeat(1_024);
    for value in [TOKEN, "tskey-auth-abcdef", longest.as_str()] {
        assert!(GuestSecret::line(value).is_ok(), "{value}");
    }
    let too_long = "a".repeat(1_025);
    for value in [
        "",
        too_long.as_str(),
        "with space",
        "with\nnewline",
        "with\0nul",
    ] {
        assert!(GuestSecret::line(value).is_err(), "{value:?}");
    }
}

#[test]
fn a_command_is_bounded_before_any_channel_carries_it() {
    let longest = "a".repeat(64 * 1_024);
    assert!(GuestCommand::quiet(&longest).is_ok());
    let too_long = "a".repeat(64 * 1_024 + 1);
    for script in ["", too_long.as_str(), "true\0false"] {
        assert!(GuestCommand::quiet(script).is_err(), "{script:?}");
    }
}

/// Captured output from a secret-carrying command is never produced, so it can
/// never reach a log.
#[test]
fn a_command_carrying_a_secret_cannot_capture_output() {
    let secret = GuestSecret::line(TOKEN).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let script = GuestProgram::Script("true");
    assert!(GuestCommand::new(script, Some(&secret), GuestCapture::Discard).is_ok());
    assert!(GuestCommand::new(script, None, GuestCapture::Bounded(256)).is_ok());
    assert!(GuestCommand::new(script, Some(&secret), GuestCapture::Bounded(256)).is_err());
}

/// The program form is the only one a channel may run as an identity other than
/// the job account, so its argv is bounded exactly like a script is.
#[test]
fn a_program_command_is_bounded_by_its_path_and_its_argv() {
    let arguments = ["--wait-seconds".to_owned(), "20".to_owned()];
    assert!(
        GuestCommand::new(
            GuestProgram::Program {
                path: "/usr/local/libexec/flanforge-guest-ready",
                arguments: &arguments,
            },
            None,
            GuestCapture::Bounded(4_096),
        )
        .is_ok()
    );
    let many = (0..17).map(|index| index.to_string()).collect::<Vec<_>>();
    let with_nul = ["a\0b".to_owned()];
    let refused: [(&str, &[String]); 5] = [
        ("relative/helper", &arguments),
        ("/usr/local/../bin/sh", &arguments),
        ("", &arguments),
        ("/usr/bin/env", &many),
        ("/usr/bin/env", &with_nul),
    ];
    for (path, arguments) in refused {
        assert!(
            GuestCommand::new(
                GuestProgram::Program { path, arguments },
                None,
                GuestCapture::Discard,
            )
            .is_err(),
            "accepted {path}"
        );
    }
}

/// A spawn names a guest-side process a channel may end without its own
/// connection, so an unnamed or unbounded one is refused before it starts.
#[test]
fn a_spawn_requires_an_identity_and_a_guest_side_deadline() {
    let script = GuestProgram::Script("true");
    let quiet = || {
        GuestCommand::new(script, None, GuestCapture::Discard)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"))
    };
    assert!(GuestSpawn::new(quiet(), uuid::Uuid::new_v4(), Duration::from_mins(1)).is_ok());
    assert!(GuestSpawn::new(quiet(), uuid::Uuid::nil(), Duration::from_mins(1)).is_err());
    assert!(GuestSpawn::new(quiet(), uuid::Uuid::new_v4(), Duration::ZERO).is_err());
    let capturing = GuestCommand::new(script, None, GuestCapture::Bounded(256))
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(GuestSpawn::new(capturing, uuid::Uuid::new_v4(), Duration::from_mins(1)).is_err());
}

#[test]
fn only_a_zero_code_succeeds_and_only_a_lost_observation_is_not_a_verdict() {
    assert!(GuestExit::Code(0).is_success());
    assert!(GuestExit::Code(0).is_verdict());
    assert!(!GuestExit::Code(7).is_success());
    assert!(GuestExit::Code(7).is_verdict());
    assert!(!GuestExit::Signal(9).is_success());
    assert!(GuestExit::Signal(9).is_verdict());
    assert!(!GuestExit::Lost.is_success());
    assert!(!GuestExit::Lost.is_verdict());
    assert_eq!(GuestExit::Code(7).code(), Some(7));
    assert_eq!(GuestExit::Signal(9).code(), None);
    assert_eq!(GuestExit::Lost.code(), None);
}
