use std::{io, path::Path, time::Duration};

use flanforge_cli::DaemonPrivArgs;

use super::{
    firewall::{AppRule, app_rule, is_globally_enabled, manual_commands},
    outcome::{ProbeOutcome, parse_probe_report},
    plan::{Gated, Plan, ensure_arguments_consistent},
    probe::removable_volume,
    report::{Gate, GateState, ensure_satisfied},
    target,
};

const LISTAPPS: &str = "ALF: total number of apps = 3 \n\
\n\
1 :  /usr/libexec/bootpd \n\
 \t ( Allow incoming connections ) \n\
\n\
2 :  /Users/service/Library/Application Support/flanforge/bin/flanforged \n\
 \t ( Block incoming connections ) \n\
\n\
3 :  /opt/homebrew/bin/tart \n\
 \t ( Allow incoming connections ) \n";

const INSTALLED: &str = "/Users/service/Library/Application Support/flanforge/bin/flanforged";

fn arguments() -> DaemonPrivArgs {
    DaemonPrivArgs {
        check: false,
        grant_firewall: false,
        prompt: false,
        elevated: None,
        probe: false,
    }
}

#[test]
fn listapps_reports_the_rule_for_a_listed_path_and_nothing_for_an_absent_one() {
    assert_eq!(
        app_rule(LISTAPPS, Path::new(INSTALLED)),
        Some(AppRule::Block)
    );
    assert_eq!(
        app_rule(LISTAPPS, Path::new("/opt/homebrew/bin/tart")),
        Some(AppRule::Allow)
    );
    // A path the firewall has never seen is dropped silently, not blocked.
    assert_eq!(
        app_rule(LISTAPPS, Path::new("/Users/service/build/flanforged")),
        None
    );
    // A prefix of a listed path is a different binary.
    assert_eq!(app_rule(LISTAPPS, Path::new("/opt/homebrew/bin")), None);
    assert_eq!(app_rule("", Path::new(INSTALLED)), None);
}

#[test]
fn the_global_firewall_state_is_read_from_either_spelling() {
    assert_eq!(
        is_globally_enabled("Firewall is enabled. (State = 1)"),
        Some(true)
    );
    assert_eq!(
        is_globally_enabled("Firewall is enabled. (State = 2)"),
        Some(true)
    );
    assert_eq!(
        is_globally_enabled("Firewall is disabled. (State = 0)"),
        Some(false)
    );
    assert_eq!(is_globally_enabled("Firewall is disabled."), Some(false));
    assert_eq!(is_globally_enabled(""), None);
}

#[test]
fn the_remediation_names_the_authorized_path_in_both_commands() {
    let commands = manual_commands(Path::new(INSTALLED));
    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|command| command.contains(INSTALLED)));
    assert!(commands[0].contains("--add"));
    assert!(commands[1].contains("--unblockapp"));
}

#[test]
fn the_removable_volume_gate_applies_only_under_volumes() {
    assert_eq!(
        removable_volume(Path::new("/Volumes/library/tart")),
        Some("/Volumes/library".into())
    );
    assert_eq!(
        removable_volume(Path::new("/Volumes/library")),
        Some("/Volumes/library".into())
    );
    assert_eq!(removable_volume(Path::new("/Users/service/.tart")), None);
    assert_eq!(removable_volume(Path::new("/Volumes")), None);
    assert_eq!(removable_volume(Path::new("/VolumesX/tart")), None);
}

#[test]
fn an_unanswered_prompt_is_distinct_from_a_refusal() {
    assert_eq!(ProbeOutcome::classify(None), ProbeOutcome::TimedOut);
    assert_eq!(
        ProbeOutcome::classify(Some(Err(io::Error::from(io::ErrorKind::PermissionDenied)))),
        ProbeOutcome::Denied
    );
    assert_eq!(
        ProbeOutcome::classify(Some(Err(io::Error::from(io::ErrorKind::HostUnreachable)))),
        ProbeOutcome::Denied
    );
    assert_eq!(ProbeOutcome::classify(Some(Ok(()))), ProbeOutcome::Allowed);
    assert!(matches!(
        ProbeOutcome::classify(Some(Err(io::Error::from(io::ErrorKind::NotFound)))),
        ProbeOutcome::Failed(_)
    ));
}

#[test]
fn delegated_probe_results_survive_the_round_trip() {
    for outcome in [
        ProbeOutcome::Allowed,
        ProbeOutcome::Denied,
        ProbeOutcome::TimedOut,
        ProbeOutcome::Skipped("not on a removable volume".to_owned()),
        ProbeOutcome::Failed("cannot load configuration".to_owned()),
    ] {
        let report = format!("volume={}\nnetwork=allowed\n", outcome.token());
        assert_eq!(parse_probe_report(&report, "volume"), Some(outcome));
        assert_eq!(
            parse_probe_report(&report, "network"),
            Some(ProbeOutcome::Allowed)
        );
    }
    // Log lines on the same stream are not results.
    assert_eq!(
        parse_probe_report("INFO something happened\nvolume=denied\n", "volume"),
        Some(ProbeOutcome::Denied)
    );
    assert_eq!(parse_probe_report("volume=nonsense\n", "volume"), None);
    assert_eq!(parse_probe_report("", "network"), None);
}

#[test]
fn a_multi_line_failure_stays_on_one_result_line() {
    let token = ProbeOutcome::Failed("first line\nsecond line".to_owned()).token();
    assert_eq!(token.lines().count(), 1);
    assert_eq!(
        ProbeOutcome::parse(&token),
        Some(ProbeOutcome::Failed("first line second line".to_owned()))
    );
}

#[test]
fn the_installed_binary_is_authorized_rather_than_the_running_one() {
    let installed = Path::new(INSTALLED);
    let current = Path::new("/Users/service/build/flanforged");

    let elsewhere = target::decide(installed, true, current);
    assert_eq!(elsewhere.binary, installed);
    assert_eq!(elsewhere.probe_via.as_deref(), Some(installed));
    assert!(
        elsewhere
            .note
            .unwrap_or_default()
            .contains("/Users/service/build/flanforged")
    );

    // Already the installed binary: nothing to delegate and nothing to explain.
    let installed_run = target::decide(installed, true, installed);
    assert_eq!(installed_run.binary, installed);
    assert_eq!(installed_run.probe_via, None);
    assert_eq!(installed_run.note, None);

    // Nothing installed yet: authorize what is running, and say so.
    let absent = target::decide(installed, false, current);
    assert_eq!(absent.binary, current);
    assert_eq!(absent.probe_via, None);
    assert!(absent.note.unwrap_or_default().contains("daemon install"));
}

#[test]
fn no_action_flag_means_report_only_and_each_action_scopes_its_own_gates() {
    let report_only = Plan::from_arguments(&arguments());
    assert!(report_only.is_report_only());
    assert!(!report_only.is_granting_firewall);
    assert!(report_only.is_in_scope(Gated::Firewall));
    assert!(report_only.is_in_scope(Gated::LocalNetwork));

    let checked = Plan::from_arguments(&DaemonPrivArgs {
        check: true,
        ..arguments()
    });
    assert_eq!(checked, report_only);

    let granted = Plan::from_arguments(&DaemonPrivArgs {
        grant_firewall: true,
        ..arguments()
    });
    assert!(granted.is_granting_firewall);
    assert!(!granted.is_prompting);
    assert!(granted.is_in_scope(Gated::Firewall));
    assert!(!granted.is_in_scope(Gated::RemovableVolume));

    let prompted = Plan::from_arguments(&DaemonPrivArgs {
        prompt: true,
        ..arguments()
    });
    assert!(!prompted.is_granting_firewall);
    assert!(prompted.is_in_scope(Gated::RemovableVolume));
    assert!(!prompted.is_in_scope(Gated::Firewall));

    // Asking for both actions covers every gate without reverting to a report.
    let both = Plan::from_arguments(&DaemonPrivArgs {
        grant_firewall: true,
        prompt: true,
        ..arguments()
    });
    assert!(!both.is_report_only());
    assert!(both.is_in_scope(Gated::Firewall));
    assert!(both.is_in_scope(Gated::RemovableVolume));
    assert!(both.is_in_scope(Gated::LocalNetwork));
}

#[test]
fn hidden_arguments_cannot_be_mixed_with_an_operator_action() {
    assert!(ensure_arguments_consistent(&arguments()).is_ok());
    assert!(
        ensure_arguments_consistent(&DaemonPrivArgs {
            elevated: Some("/Users/service/bin/flanforged".into()),
            check: true,
            ..arguments()
        })
        .is_err()
    );
    assert!(
        ensure_arguments_consistent(&DaemonPrivArgs {
            elevated: Some("/Users/service/bin/flanforged".into()),
            prompt: true,
            ..arguments()
        })
        .is_err()
    );
    assert!(
        ensure_arguments_consistent(&DaemonPrivArgs {
            elevated: Some("/Users/service/bin/flanforged".into()),
            ..arguments()
        })
        .is_ok()
    );
    assert!(
        ensure_arguments_consistent(&DaemonPrivArgs {
            probe: true,
            grant_firewall: true,
            ..arguments()
        })
        .is_err()
    );
    assert!(
        ensure_arguments_consistent(&DaemonPrivArgs {
            probe: true,
            prompt: true,
            ..arguments()
        })
        .is_ok()
    );
}

#[test]
fn only_gates_in_scope_decide_the_exit_status() {
    let gates = vec![
        Gate::new(Gated::Firewall, GateState::Satisfied, "allowed"),
        Gate::new(Gated::RemovableVolume, GateState::Missing, "unanswered"),
        Gate::new(Gated::LocalNetwork, GateState::NotApplicable, "skipped"),
    ];
    let report_only = Plan::from_arguments(&arguments());
    assert!(ensure_satisfied(&gates, report_only).is_err());

    let granted = Plan::from_arguments(&DaemonPrivArgs {
        grant_firewall: true,
        ..arguments()
    });
    assert!(ensure_satisfied(&gates, granted).is_ok());

    let prompted = Plan::from_arguments(&DaemonPrivArgs {
        prompt: true,
        ..arguments()
    });
    assert!(ensure_satisfied(&gates, prompted).is_err());

    // An unknown gate is not a satisfied one.
    let unknown = vec![Gate::new(Gated::Firewall, GateState::Unknown, "unreadable")];
    assert!(ensure_satisfied(&unknown, granted).is_err());
}

#[test]
fn a_satisfied_gate_carries_no_remediation() {
    let satisfied = Gate::new(Gated::Firewall, GateState::Satisfied, "allowed")
        .with_fixes(manual_commands(Path::new(INSTALLED)));
    assert!(satisfied.fixes.is_empty());
    let missing = Gate::new(Gated::Firewall, GateState::Missing, "not listed")
        .with_fixes(manual_commands(Path::new(INSTALLED)));
    assert_eq!(missing.fixes.len(), 2);
}

#[test]
fn probe_bounds_stay_short_enough_to_report_rather_than_hang() {
    assert!(super::probe::CHECK_TIMEOUT <= Duration::from_secs(10));
    assert!(super::probe::PROMPT_TIMEOUT > super::probe::CHECK_TIMEOUT);
}
