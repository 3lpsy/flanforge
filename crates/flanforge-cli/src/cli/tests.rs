use std::path::PathBuf;

use clap::Parser;

use super::*;

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
    assert!(!defaults.check && !defaults.grant_firewall && !defaults.grant_all);
    assert!(!defaults.prompt && !defaults.probe && defaults.elevated.is_none());

    let granted = parse(&["--grant-firewall"]);
    assert!(granted.grant_firewall && !granted.grant_all);
    assert!(parse(&["--grant-all", "--prompt"]).grant_all);

    // Reporting and acting are separate, deliberate invocations.
    for rejected in [
        vec!["--check", "--grant-all"],
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
