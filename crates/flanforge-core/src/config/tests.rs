use std::{collections::BTreeMap, str::FromStr};

use url::Url;

use super::*;
use crate::ProfileName;

fn valid_config() -> Config {
    let source = r#"
        [server]
        listen = "127.0.0.1:9843"
        request_body_limit_bytes = 4096
        allocation_wait_seconds = 600
        shutdown_grace_seconds = 30

        [oidc]
        issuer = "https://forgejo.example/api/actions"
        audience = "flanforged"
        jwks_url = "https://forgejo.example/api/actions/.well-known/jwks"

        [forgejo]
        api_url = "https://forgejo.example/api/v1/"
        api_token_file = "/Users/runner/Library/Application Support/flanforge/token"

        [runtime]
        state_dir = "/Users/runner/Library/Application Support/flanforge/state"
        tart_path = "/opt/homebrew/bin/tart"
        ssh_path = "/usr/bin/ssh"
        scp_path = "/usr/bin/scp"
        forgejo_runner_host_path = "/Users/runner/Library/Application Support/flanforge/bin/forgejo-runner"
        vm_prefix = "ci-"
        max_running_vms = 2

        [guest]
        ssh_user = "admin"
        ssh_identity_file = "/Users/runner/.ssh/tart_ci"
        ssh_known_hosts_file = "/Users/runner/.ssh/tart_known_hosts"
        ssh_host_key_alias = "tart-ci"
        forgejo_runner_path = "/usr/local/bin/forgejo-runner"

        [profiles.halogen]
        repository = "owner/halogen"
        template = "flanforge-base"
        runner_label = "macos-tart-halogen"
        job_name = "apple-build"
        allowed_workflows = ["apple.yml"]
        allowed_events = ["workflow_dispatch"]
        allowed_refs = ["refs/heads/main"]
        allowed_ref_prefixes = ["refs/tags/v"]
        require_protected_ref = true
        network = "softnet"
        cpu_count = 8
        memory_mb = 12288
        boot_timeout_seconds = 300
        idle_timeout_seconds = 600
        job_timeout_seconds = 7200
        cleanup_timeout_seconds = 120
    "#;
    toml::from_str(source).unwrap_or_else(|error| unreachable!("{error}"))
}

#[test]
fn complete_configuration_is_valid() {
    assert_eq!(valid_config().ensure_valid(), Ok(()));
}

#[test]
fn logging_defaults_to_stdout_info_and_validates_file_settings() {
    let mut config = valid_config();
    assert_eq!(config.logging.level, "info");
    assert_eq!(config.logging.path, None);

    config.logging.level = "debug".into();
    config.logging.path = Some("/private/var/log/flanforged.log".into());
    assert_eq!(config.ensure_valid(), Ok(()));

    config.logging.level = "verbose".into();
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "logging.level"
        })
    ));

    config.logging.level = "info".into();
    config.logging.path = Some("relative.log".into());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidPath {
            field: "logging.path"
        })
    ));
}

#[test]
fn quoted_known_hosts_path_is_rejected() {
    let mut config = valid_config();
    config.guest.ssh_known_hosts_file = Some("/private/known\"_hosts".into());
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidPath {
            field: "guest.ssh_known_hosts_file"
        })
    );
    config.guest.ssh_known_hosts_file = Some("/private/known\\_hosts".into());
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidPath {
            field: "guest.ssh_known_hosts_file"
        })
    );
}

#[test]
fn host_key_verification_defaults_to_enabled_and_requires_its_anchor() {
    let mut config = valid_config();
    assert!(config.guest.verify_host_key);
    assert_eq!(config.ensure_valid(), Ok(()));

    config.guest.ssh_host_key_alias = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingGuestSetting {
            field: "ssh_host_key_alias"
        })
    );
    config.guest.ssh_host_key_alias = Some("tart-ci".into());
    config.guest.ssh_known_hosts_file = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingGuestSetting {
            field: "ssh_known_hosts_file"
        })
    );
}

#[test]
fn disabled_host_key_verification_does_not_require_or_validate_the_anchor() {
    let source = r#"
        verify_host_key = false
        ssh_user = "admin"
        ssh_identity_file = "/Users/runner/.ssh/tart_ci"
        forgejo_runner_path = "/usr/local/bin/forgejo-runner"
    "#;
    let mut config = valid_config();
    config.guest = toml::from_str(source).unwrap_or_else(|error| unreachable!("{error}"));
    assert!(!config.guest.verify_host_key);
    assert_eq!(config.guest.ssh_known_hosts_file, None);
    assert_eq!(config.ensure_valid(), Ok(()));

    config.guest.ssh_known_hosts_file = Some("relative/known\"_hosts".into());
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn public_listen_address_is_rejected() {
    let mut config = valid_config();
    config.server.listen = "0.0.0.0:9843"
        .parse()
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(config.ensure_valid(), Err(ConfigError::UnsafeListenAddress));
}

#[test]
fn service_owned_template_is_rejected() {
    let mut config = valid_config();
    let profile = config
        .profiles
        .get_mut(&ProfileName::from_str("halogen").unwrap_or_else(|error| unreachable!("{error}")))
        .unwrap_or_else(|| unreachable!());
    profile.template =
        crate::VmName::new("ci-attacker").unwrap_or_else(|error| unreachable!("{error}"));
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
}

#[test]
fn duplicate_repository_binding_is_rejected() {
    let mut config = valid_config();
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!());
    let mut duplicate = profile;
    duplicate.runner_label =
        crate::RunnerLabel::new("another-label").unwrap_or_else(|error| unreachable!("{error}"));
    config.profiles = BTreeMap::from([
        (
            ProfileName::new("one").unwrap_or_else(|error| unreachable!("{error}")),
            duplicate.clone(),
        ),
        (
            ProfileName::new("two").unwrap_or_else(|error| unreachable!("{error}")),
            duplicate,
        ),
    ]);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::DuplicateProfileBinding)
    );
}

#[test]
fn config_rejects_unknown_fields() {
    let source = "[server]\nlisten='127.0.0.1:1'\nunknown=true";
    assert!(toml::from_str::<Config>(source).is_err());
}

#[test]
fn configuration_rejects_an_oversized_generated_vm_name() {
    let mut config = valid_config();
    config.runtime.vm_prefix = crate::VmPrefix::new("abcdefghijklmnopqrstuvw-")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let old_name = ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}"));
    let profile = config
        .profiles
        .remove(&old_name)
        .unwrap_or_else(|| unreachable!());
    let long_name = ProfileName::new("abcdefghijklmnopqrstuvwxzy123456")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    config.profiles.insert(long_name, profile);
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
}

#[test]
fn configuration_rejects_an_oversized_per_allocation_label() {
    let mut config = valid_config();
    let profile = config
        .profiles
        .values_mut()
        .next()
        .unwrap_or_else(|| unreachable!());
    profile.runner_label = crate::RunnerLabel::new("abcdefghijklmnopqrstuvwxyz12")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
}

#[test]
fn tailscale_is_disabled_by_default() {
    let config = valid_config();
    assert!(!config.tailscale.enabled);
    assert!(config.tailscale.preauth_key_file.is_none());
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn enabled_tailscale_requires_host_credential_and_login_server() {
    let mut config = valid_config();
    config.tailscale.enabled = true;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::MissingTailscaleSetting {
            field: "preauth_key_file"
        })
    ));

    config.tailscale.preauth_key_file = Some("/private/flanforge/headscale-key".into());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::MissingTailscaleSetting {
            field: "login_server"
        })
    ));

    config.tailscale.login_server = Some(
        Url::parse("https://headscale.example")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn tailscale_arguments_are_parsed_without_shell_evaluation() {
    for argument in ["--accept-dns='unterminated", "--accept-dns=true\ntouch"] {
        let mut config = valid_config();
        config.tailscale.extra_args = argument.to_owned();
        assert_eq!(
            config.ensure_valid(),
            Err(ConfigError::InvalidTailscaleArguments),
            "accepted {argument}"
        );
    }

    let mut config = valid_config();
    config.tailscale.extra_args =
        "--accept-dns=false --advertise-tags='tag:ci,tag:apple builder' --auth-key=override".into();
    assert_eq!(config.ensure_valid(), Ok(()));
    assert_eq!(
        config
            .tailscale
            .extra_arguments()
            .unwrap_or_else(|error| unreachable!("parse: {error}")),
        [
            "--accept-dns=false",
            "--advertise-tags=tag:ci,tag:apple builder",
            "--auth-key=override",
        ]
    );
}

#[test]
fn tailscale_hostname_and_login_server_are_validated() {
    let mut config = valid_config();
    config.tailscale.hostname = Some("ci-guest.example".into());
    config.tailscale.login_server = Some(
        Url::parse("https://headscale.example")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    assert_eq!(config.ensure_valid(), Ok(()));

    config.tailscale.hostname = Some("ci;touch-host".into());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "tailscale.hostname"
        })
    ));

    config.tailscale.hostname = None;
    config.tailscale.login_server = Some(
        Url::parse("https://headscale.example?override=true")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "tailscale.login_server"
        })
    ));
}

fn profile_mut(config: &mut Config) -> &mut Profile {
    config
        .profiles
        .values_mut()
        .next()
        .unwrap_or_else(|| unreachable!("fixture profile"))
}

fn warm_config() -> Config {
    let mut config = valid_config();
    config.runtime.tart_home = Some("/Users/runner/.tart".into());
    let profile = profile_mut(&mut config);
    profile.warm_template = crate::VmName::new("halogen-warm").ok();
    profile.regeneration_workflow = Some("apple.yml".to_owned());
    config
}

#[test]
fn existing_config_loads_without_the_new_fields() {
    let config = valid_config();
    assert_eq!(config.runtime.reap_interval_hours, 168);
    assert_eq!(config.runtime.host_cpu_count, None);
    assert_eq!(config.runtime.host_memory_mb, None);
    let profile = config
        .profiles
        .values()
        .next()
        .unwrap_or_else(|| unreachable!("fixture profile"));
    assert_eq!(profile.warm_template, None);
    assert_eq!(profile.regeneration_workflow, None);
    assert!(profile.reap);
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn warm_template_rules_reject_prefix_collision_length_and_half_declaration() {
    assert_eq!(warm_config().ensure_valid(), Ok(()));

    let mut config = warm_config();
    profile_mut(&mut config).warm_template = crate::VmName::new("ci-halogen-warm").ok();
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let mut config = warm_config();
    profile_mut(&mut config).warm_template = crate::VmName::new("w".repeat(72)).ok();
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let mut config = warm_config();
    profile_mut(&mut config).warm_template = crate::VmName::new("w".repeat(71)).ok();
    assert_eq!(config.ensure_valid(), Ok(()));

    let mut config = warm_config();
    profile_mut(&mut config).regeneration_workflow = None;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let mut config = warm_config();
    profile_mut(&mut config).warm_template = None;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
}

#[test]
fn warm_names_must_be_unique_and_disjoint_from_every_template() {
    let mut config = warm_config();
    let mut second = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!("fixture profile"));
    second.repository =
        crate::RepositoryName::new("owner/liftfg").unwrap_or_else(|error| unreachable!("{error}"));
    second.runner_label = crate::RunnerLabel::new("macos-tart-liftfg")
        .unwrap_or_else(|error| unreachable!("{error}"));
    let name = ProfileName::new("liftfg").unwrap_or_else(|error| unreachable!("{error}"));
    config.profiles.insert(name.clone(), second);
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    // A second profile's template may not be another profile's rollback name.
    let second = config
        .profiles
        .get_mut(&name)
        .unwrap_or_else(|| unreachable!("fixture profile"));
    second.warm_template = crate::VmName::new("liftfg-warm").ok();
    second.template =
        crate::VmName::new("halogen-warm.previous").unwrap_or_else(|error| unreachable!("{error}"));
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let second = config
        .profiles
        .get_mut(&name)
        .unwrap_or_else(|| unreachable!("fixture profile"));
    second.template =
        crate::VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn regeneration_workflow_must_be_an_allowed_workflow() {
    let mut config = warm_config();
    profile_mut(&mut config).regeneration_workflow = Some("warm.yml".to_owned());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let mut config = warm_config();
    profile_mut(&mut config).regeneration_workflow = Some("../escape.yml".to_owned());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    let mut config = warm_config();
    let profile = profile_mut(&mut config);
    profile.allowed_workflows.insert("warm.yml".to_owned());
    profile.regeneration_workflow = Some("warm.yml".to_owned());
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn host_budget_must_be_whole_and_cover_every_profile_ceiling() {
    let mut config = valid_config();
    config.runtime.host_cpu_count = Some(16);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::OutOfRange {
            field: "runtime.host_cpu_count"
        })
    );

    let mut config = valid_config();
    config.runtime.host_memory_mb = Some(32_768);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::OutOfRange {
            field: "runtime.host_memory_mb"
        })
    );

    let mut config = valid_config();
    config.runtime.host_cpu_count = Some(16);
    config.runtime.host_memory_mb = Some(32_768);
    assert_eq!(config.ensure_valid(), Ok(()));

    config.runtime.host_cpu_count = Some(4);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile {
            profile: "halogen".to_owned(),
            message: "profile is larger than the host budget and can never be admitted".to_owned(),
        })
    );

    config.runtime.host_cpu_count = Some(16);
    config.runtime.host_memory_mb = Some(2_047);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::OutOfRange {
            field: "runtime.host_memory_mb"
        })
    );
}

#[test]
fn reap_interval_is_bounded_and_zero_disables_the_sweep() {
    let mut config = valid_config();
    config.runtime.reap_interval_hours = 0;
    assert_eq!(config.ensure_valid(), Ok(()));

    config.runtime.reap_interval_hours = 8_761;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::OutOfRange {
            field: "runtime.reap_interval_hours"
        })
    );
}

#[test]
fn tart_home_is_required_once_a_profile_declares_a_warm_template() {
    let mut config = warm_config();
    config.runtime.tart_home = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingRuntimeSetting { field: "tart_home" })
    );

    let mut config = valid_config();
    config.runtime.tart_home = None;
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn restart_only_differences_names_every_restart_only_field() {
    let old = valid_config();
    assert!(restart_only_differences(&old, &old).is_empty());

    // Everything a reload applies: profiles, logging, the host budget, and the
    // sweep period the reaper re-reads each cycle.
    let mut new = valid_config();
    new.logging.level = "debug".into();
    profile_mut(&mut new).cpu_count = 2;
    new.runtime.reap_interval_hours = 24;
    new.runtime.host_cpu_count = Some(8);
    new.runtime.host_memory_mb = Some(16_384);
    assert!(restart_only_differences(&old, &new).is_empty());

    let mut new = valid_config();
    new.server.allocation_wait_seconds += 1;
    new.runtime.poll_seconds += 1;
    new.runtime.max_running_vms = 1;
    new.runtime.state_dir = "/private/state".into();
    new.server.listen = "127.0.0.1:9999"
        .parse()
        .unwrap_or_else(|error| unreachable!("{error}"));
    new.forgejo.api_token_file = "/private/token".into();
    assert_eq!(
        restart_only_differences(&old, &new),
        vec![
            "server.listen",
            "server.allocation_wait_seconds",
            "forgejo.api_token_file",
            "runtime.state_dir",
            "runtime.max_running_vms",
            "runtime.poll_seconds",
        ]
    );
}

#[test]
fn only_re_enabling_the_sweep_is_reported_as_pending_a_restart() {
    let mut disabled = valid_config();
    disabled.runtime.reap_interval_hours = 0;
    let mut enabled = valid_config();
    enabled.runtime.reap_interval_hours = 168;
    let mut shortened = valid_config();
    shortened.runtime.reap_interval_hours = 24;

    assert_eq!(
        restart_only_differences(&disabled, &enabled),
        vec!["runtime.reap_interval_hours"]
    );
    assert!(restart_only_differences(&enabled, &shortened).is_empty());
    assert!(restart_only_differences(&enabled, &disabled).is_empty());
}
