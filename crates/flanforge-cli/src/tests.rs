use std::path::PathBuf;

use clap::Parser;

use super::*;
use crate::{daemon::DaemonArgs, hot::HotCommand};

#[test]
fn daemon_is_available_only_through_a_subcommand() {
    let arguments = Arguments::try_parse_from(["flanforged", "daemon", "run"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    assert!(matches!(
        arguments.command,
        TopCommand::Daemon(DaemonArgs {
            command: DaemonCommand::Run
        })
    ));
    assert!(Arguments::try_parse_from(["flanforged"]).is_err());
}

#[test]
fn standalone_libvirt_commands_expose_only_server_owned_controls() {
    let inspect = Arguments::try_parse_from([
        "flanforged",
        "image",
        "inspect",
        "--image",
        "/tmp/base.qcow2",
        "--manifest",
        "/tmp/base.json",
    ]);
    assert!(inspect.is_ok());
    let import = Arguments::try_parse_from([
        "flanforged",
        "image",
        "import",
        "--name",
        "flanforge-base",
        "--image",
        "/tmp/base.qcow2",
        "--manifest",
        "/tmp/base.json",
    ]);
    assert!(import.is_ok());
    let smoke =
        Arguments::try_parse_from(["flanforged", "runtime", "smoke", "--profile", "project"]);
    assert!(smoke.is_ok());
    assert!(Arguments::try_parse_from(["flanforged", "daemon", "doctor"]).is_ok());

    for rejected in [
        vec!["runtime", "smoke", "--profile", "project", "--keep"],
        vec!["runtime", "smoke", "--profile", "project", "--cpu", "8"],
        vec![
            "runtime",
            "smoke",
            "--profile",
            "project",
            "--uri",
            "qemu:///session",
        ],
        vec![
            "image",
            "import",
            "--name",
            "base",
            "--image",
            "/tmp/base.qcow2",
            "--manifest",
            "/tmp/base.json",
            "--force",
        ],
    ] {
        let mut arguments = vec!["flanforged"];
        arguments.extend(rejected);
        assert!(Arguments::try_parse_from(arguments).is_err());
    }
}

#[test]
fn logs_defaults_to_a_bounded_tail_and_accepts_follow() {
    let arguments = Arguments::try_parse_from(["flanforged", "daemon", "logs"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Daemon(DaemonArgs {
        command: DaemonCommand::Logs(logs),
    }) = arguments.command
    else {
        unreachable!("daemon logs");
    };
    assert_eq!(logs.lines, 50);
    assert!(!logs.follow && !logs.stderr);

    let arguments = Arguments::try_parse_from(["flanforged", "daemon", "logs", "-f", "-n", "200"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Daemon(DaemonArgs {
        command: DaemonCommand::Logs(logs),
    }) = arguments.command
    else {
        unreachable!("daemon logs");
    };
    assert!(logs.follow);
    assert_eq!(logs.lines, 200);

    assert!(Arguments::try_parse_from(["flanforged", "daemon", "logs", "-n", "0"]).is_err());
}

// Both reports run unless silenced, and the two are silenced independently: a
// deploy re-keys the macOS gates, so that is exactly when they matter (CLI-611).
#[test]
fn start_and_restart_report_by_default_and_silence_each_half_separately() {
    let parse = |command: &str, extra: &[&str]| {
        let mut arguments = vec!["flanforged", "daemon", command];
        arguments.extend_from_slice(extra);
        let arguments = Arguments::try_parse_from(arguments)
            .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Daemon(daemon) = arguments.command else {
            unreachable!("daemon command")
        };
        match daemon.command {
            DaemonCommand::Start(control) | DaemonCommand::Restart(control) => control,
            _ => unreachable!("control command"),
        }
    };

    for command in ["start", "restart"] {
        let defaults = parse(command, &[]);
        assert!(!defaults.no_status && !defaults.no_privcheck, "{command}");

        let quiet = parse(command, &["--no-status"]);
        assert!(quiet.no_status && !quiet.no_privcheck, "{command}");

        let ungated = parse(command, &["--no-privcheck"]);
        assert!(!ungated.no_status && ungated.no_privcheck, "{command}");

        let silent = parse(command, &["--no-status", "--no-privcheck"]);
        assert!(silent.no_status && silent.no_privcheck, "{command}");
    }
}

#[test]
fn priv_reports_by_default_and_refuses_check_alongside_an_action() {
    let parse = |extra: &[&str]| {
        let mut arguments = vec!["flanforged", "daemon", "priv"];
        arguments.extend_from_slice(extra);
        let arguments = Arguments::try_parse_from(arguments)
            .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Daemon(daemon) = arguments.command else {
            unreachable!("daemon command")
        };
        let DaemonCommand::Priv(gates) = daemon.command else {
            unreachable!("priv command")
        };
        gates
    };

    let defaults = parse(&[]);
    assert!(!defaults.check && !defaults.grant_firewall);
    assert!(!defaults.prompt && !defaults.probe && defaults.elevated.is_none());

    let granted = parse(&["--grant-firewall"]);
    assert!(granted.grant_firewall && !granted.prompt);

    // The firewall grant and the consent prompts are independent actions.
    let both = parse(&["--grant-firewall", "--prompt"]);
    assert!(both.grant_firewall && both.prompt);

    // Reporting and acting are separate, deliberate invocations.
    for rejected in [
        vec!["--check", "--grant-firewall"],
        vec!["--check", "--prompt"],
    ] {
        let mut arguments = vec!["flanforged", "daemon", "priv"];
        arguments.extend_from_slice(&rejected);
        assert!(
            Arguments::try_parse_from(arguments).is_err(),
            "accepted {rejected:?}"
        );
    }
}

#[test]
fn global_config_override_is_accepted_after_subcommands() {
    let arguments = Arguments::try_parse_from([
        "flanforged",
        "config",
        "view",
        "--logging",
        "--config",
        "/private/config.toml",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    assert_eq!(
        arguments.config,
        Some(PathBuf::from("/private/config.toml"))
    );
}

#[test]
fn typed_config_overrides_are_global_and_structurally_validated() {
    let arguments = Arguments::try_parse_from([
        "flanforged",
        "daemon",
        "run",
        "--set",
        "runtime.backend.kind=libvirt",
        "--set",
        "runtime.backend.qemu_img_path=/opt/qemu-img",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    assert_eq!(arguments.overrides.len(), 2);
    assert_eq!(arguments.overrides[0].key(), "runtime.backend.kind");
    assert!(
        Arguments::try_parse_from([
            "flanforged",
            "daemon",
            "run",
            "--set",
            "runtime..kind=libvirt",
        ])
        .is_err()
    );
}

#[test]
fn config_view_rejects_multiple_sections() {
    assert!(
        Arguments::try_parse_from(["flanforged", "config", "view", "--logging", "--server",])
            .is_err()
    );
}

#[test]
fn config_view_accepts_tailscale_section() {
    let arguments = Arguments::try_parse_from(["flanforged", "config", "view", "--tailscale"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Config(config) = arguments.command else {
        unreachable!("config command")
    };
    let ConfigCommand::View(view) = config.command else {
        unreachable!("view command")
    };
    assert!(view.tailscale);
}

#[test]
fn config_generate_defaults_to_the_selected_path_and_refuses_overwrites() {
    let arguments = Arguments::try_parse_from(["flanforged", "config", "generate"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Config(config) = arguments.command else {
        unreachable!("config command")
    };
    let ConfigCommand::Generate(generate) = config.command else {
        unreachable!("generate command")
    };
    assert_eq!(generate.output, None);
    assert!(!generate.force);
    assert_eq!(generate.backend, ConfigBackend::default());

    let arguments = Arguments::try_parse_from([
        "flanforged",
        "config",
        "generate",
        "-o",
        "/private/config.toml",
        "--force",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Config(config) = arguments.command else {
        unreachable!("config command")
    };
    let ConfigCommand::Generate(generate) = config.command else {
        unreachable!("generate command")
    };
    assert_eq!(generate.output, Some(PathBuf::from("/private/config.toml")));
    assert!(generate.force);
}

#[test]
fn config_generate_preserves_an_explicit_backend() {
    for (name, expected) in [
        ("tart", ConfigBackend::Tart),
        ("libvirt", ConfigBackend::Libvirt),
    ] {
        let arguments =
            Arguments::try_parse_from(["flanforged", "config", "generate", "--backend", name])
                .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Config(config) = arguments.command else {
            unreachable!("config command")
        };
        let ConfigCommand::Generate(generate) = config.command else {
            unreachable!("generate command")
        };
        assert_eq!(generate.backend, expected);
    }
}

#[test]
fn cli_overrides_can_clear_a_valid_string() {
    let arguments = Arguments::try_parse_from([
        "flanforged",
        "daemon",
        "run",
        "--set",
        "tailscale.extra_args=",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    assert_eq!(arguments.overrides[0].value(), "");
}

#[test]
fn profile_create_accepts_the_complete_policy_surface() {
    let arguments = Arguments::try_parse_from([
        "flanforged",
        "profile",
        "create",
        "--profile",
        "halogen",
        "--repository",
        "owner/halogen",
        "--template",
        "flanforge-base",
        "--runner-label",
        "macos-halogen",
        "--job-name",
        "apple-build",
        "--allowed-workflow",
        "release.yml",
        "--allowed-event",
        "workflow_dispatch",
        "--allowed-ref",
        "refs/heads/main",
        "--network",
        "softnet",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    assert!(matches!(arguments.command, TopCommand::Profile(_)));
}

#[test]
fn profile_create_defaults_warm_off_and_reaping_on() {
    let create = |extra: &[&str]| {
        let mut arguments = vec![
            "flanforged",
            "profile",
            "create",
            "--profile",
            "halogen",
            "--repository",
            "owner/halogen",
            "--template",
            "flanforge-base",
            "--runner-label",
            "macos-halogen",
            "--job-name",
            "apple-build",
            "--allowed-workflow",
            "release.yml",
            "--allowed-event",
            "workflow_dispatch",
            "--allowed-ref",
            "refs/heads/main",
        ];
        arguments.extend_from_slice(extra);
        let arguments = Arguments::try_parse_from(arguments)
            .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Profile(profile) = arguments.command else {
            unreachable!("profile command")
        };
        let ProfileCommand::Create(create) = profile.command else {
            unreachable!("create command")
        };
        create
    };

    let defaults = create(&[]);
    assert_eq!(defaults.warm_template, None);
    assert_eq!(defaults.regeneration_workflow, None);
    assert!(defaults.reap);

    let warm = create(&[
        "--warm-template",
        "halogen-warm",
        "--regeneration-workflow",
        "warm.yml",
        "--reap",
        "false",
    ]);
    assert_eq!(
        warm.warm_template.map(|name| name.to_string()),
        Some("halogen-warm".to_owned())
    );
    assert_eq!(warm.regeneration_workflow.as_deref(), Some("warm.yml"));
    assert!(!warm.reap);
}

#[test]
fn allocation_list_and_cancel_take_only_an_id() {
    let arguments = Arguments::try_parse_from(["flanforged", "allocation", "list", "--json"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Allocation(allocation) = arguments.command else {
        unreachable!("allocation command")
    };
    let AllocationCommand::List(list) = allocation.command else {
        unreachable!("list command")
    };
    assert!(list.json);

    let arguments = Arguments::try_parse_from([
        "flanforged",
        "allocation",
        "cancel",
        "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
    ])
    .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Allocation(allocation) = arguments.command else {
        unreachable!("allocation command")
    };
    assert!(matches!(allocation.command, AllocationCommand::Cancel(_)));

    // There is no operator path that creates an allocation or edits a profile.
    assert!(Arguments::try_parse_from(["flanforged", "allocation", "create"]).is_err());
    assert!(
        Arguments::try_parse_from(["flanforged", "allocation", "cancel", "not-a-uuid"]).is_err()
    );
}

#[test]
fn hot_takes_a_listing_and_two_retirements_by_vm_name() {
    let arguments = Arguments::try_parse_from(["flanforged", "hot", "list", "--json"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Hot(hot) = arguments.command else {
        unreachable!("hot command")
    };
    let HotCommand::List(list) = hot.command else {
        unreachable!("hot list")
    };
    assert!(list.json);

    for (verb, is_evict) in [("drain", false), ("evict", true)] {
        let arguments = Arguments::try_parse_from(["flanforged", "hot", verb, "ci-project-1-1"])
            .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
        let TopCommand::Hot(hot) = arguments.command else {
            unreachable!("hot command")
        };
        match hot.command {
            HotCommand::Drain(arguments) => {
                assert!(!is_evict);
                assert_eq!(arguments.vm_name.as_str(), "ci-project-1-1");
            }
            HotCommand::Evict(arguments) => {
                assert!(is_evict);
                assert_eq!(arguments.vm_name.as_str(), "ci-project-1-1");
            }
            HotCommand::List(_) => unreachable!("{verb} is not a listing"),
        }
    }

    // The name is validated by its own type, so a value no VM could carry is a
    // parse error rather than a request the daemon has to refuse.
    assert!(Arguments::try_parse_from(["flanforged", "hot", "evict", "../escape"]).is_err());
    assert!(Arguments::try_parse_from(["flanforged", "hot", "drain"]).is_err());
    assert!(Arguments::try_parse_from(["flanforged", "hot", "create"]).is_err());
}

#[test]
fn reaper_run_defaults_to_a_dry_run() {
    let arguments = Arguments::try_parse_from(["flanforged", "reaper", "run"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Reaper(reaper) = arguments.command else {
        unreachable!("reaper command")
    };
    let ReaperCommand::Run(run) = reaper.command;
    assert!(!run.delete);

    let arguments = Arguments::try_parse_from(["flanforged", "reaper", "run", "--delete"])
        .unwrap_or_else(|error| unreachable!("valid CLI: {error}"));
    let TopCommand::Reaper(reaper) = arguments.command else {
        unreachable!("reaper command")
    };
    let ReaperCommand::Run(run) = reaper.command;
    assert!(run.delete);
}
