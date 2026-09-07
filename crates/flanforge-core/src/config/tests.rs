use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use url::Url;

use super::*;
use crate::{HotLanePolicy, ProfileName};

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
        runner_user = "admin"
        forgejo_runner_path = "/usr/local/bin/forgejo-runner"

        [guest.ssh]
        identity_file = "/Users/runner/.ssh/tart_ci"
        known_hosts_file = "/Users/runner/.ssh/tart_known_hosts"
        host_key_alias = "tart-ci"

        [profiles.halogen]
        repository = "owner/halogen"
        template = "flanforge-base"
        runner_label = "macos-tart-halogen"
        job_name = "apple-build"
        allowed_workflows = ["apple.yml"]
        allowed_events = ["workflow_dispatch"]
        allowed_refs = ["refs/heads/main", "refs/tags/v*"]
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

fn ssh(config: &Config) -> &GuestSshConfig {
    config
        .guest
        .ssh
        .as_ref()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"))
}

fn ssh_mut(config: &mut Config) -> &mut GuestSshConfig {
    config
        .guest
        .ssh
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"))
}

fn profile_name(name: &str) -> ProfileName {
    ProfileName::new(name).unwrap_or_else(|error| unreachable!("{error}"))
}

fn profile_from(source: &str) -> Profile {
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
fn db_path_defaults_beside_the_config_file_and_validates_when_set() {
    let mut config = valid_config();
    assert_eq!(config.db.db_path, None);
    assert_eq!(
        config
            .db
            .resolved_db_path(std::path::Path::new("/etc/flanforge/config.toml")),
        std::path::PathBuf::from("/etc/flanforge/flanforge.db")
    );

    config.db.db_path = Some("/var/lib/flanforge/flanforge.db".into());
    assert_eq!(config.ensure_valid(), Ok(()));
    assert_eq!(
        config
            .db
            .resolved_db_path(std::path::Path::new("/etc/flanforge/config.toml")),
        std::path::PathBuf::from("/var/lib/flanforge/flanforge.db")
    );

    config.db.db_path = Some("relative/flanforge.db".into());
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidPath {
            field: "db.db_path"
        })
    );
}

#[test]
fn db_path_needs_daemon_install_to_apply() {
    assert!(is_service_definition_field("db.db_path"));
}

#[test]
fn webui_defaults_are_enabled_private_and_authdb_backed() {
    let config = valid_config();
    let webui = &config.webui;
    assert!(webui.enabled);
    assert!(!webui.public_read_only);
    assert_eq!(webui.session_ttl_seconds, 604_800);
    assert_eq!(webui.request_body_limit_bytes, 65_536);
    assert!(webui.authdb.enabled);
    assert!(!webui.oidc.enabled);
    assert_eq!(webui.oidc.scopes, ["openid", "profile", "email"]);
    assert_eq!(config.ensure_valid(), Ok(()));
}

fn enabled_webui_oidc() -> WebuiOidcConfig {
    WebuiOidcConfig {
        enabled: true,
        issuer: Some(
            "https://idp.example"
                .parse()
                .unwrap_or_else(|error| unreachable!("{error}")),
        ),
        client_id: Some("flanforge-webui".to_owned()),
        client_secret_file: Some("/private/webui-oidc-secret".into()),
        redirect_url: Some(
            "https://ci.example/api/v1/oidc/callback"
                .parse()
                .unwrap_or_else(|error| unreachable!("{error}")),
        ),
        scopes: WebuiOidcConfig::default().scopes,
    }
}

#[test]
fn enabled_webui_oidc_requires_its_relying_party_settings() {
    let mut config = valid_config();
    config.webui.oidc = enabled_webui_oidc();
    assert_eq!(config.ensure_valid(), Ok(()));

    config.webui.oidc.issuer = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingWebuiOidcSetting { field: "issuer" })
    );

    config.webui.oidc = enabled_webui_oidc();
    config.webui.oidc.client_secret_file = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingWebuiOidcSetting {
            field: "client_secret_file"
        })
    );

    // A disabled table validates whatever is present but requires nothing.
    config.webui.oidc = WebuiOidcConfig::default();
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn webui_oidc_redirect_and_scopes_are_validated() {
    let mut config = valid_config();
    config.webui.oidc = enabled_webui_oidc();
    config.webui.oidc.redirect_url = Some(
        "https://ci.example/elsewhere"
            .parse()
            .unwrap_or_else(|error| unreachable!("{error}")),
    );
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidWebuiRedirectUrl)
    );

    config.webui.oidc = enabled_webui_oidc();
    config.webui.oidc.issuer = Some(
        "http://idp.example"
            .parse()
            .unwrap_or_else(|error| unreachable!("{error}")),
    );
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InsecureUrl {
            field: "webui.oidc.issuer"
        })
    );

    config.webui.oidc = enabled_webui_oidc();
    config.webui.oidc.scopes = vec!["profile".to_owned()];
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "webui.oidc.scopes"
        })
    );
}

#[test]
fn webui_advisories_flag_a_locked_out_ui_and_a_public_listener() {
    let mut config = valid_config();
    config.webui.authdb.enabled = false;
    assert!(
        config
            .advisories()
            .contains(&ConfigAdvisory::WebuiNoAuthSource)
    );

    let mut config = valid_config();
    config.webui.public_read_only = true;
    assert!(config.advisories().is_empty(), "loopback listen is private");
    config.server.listen = "0.0.0.0:9843"
        .parse()
        .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(
        config.advisories(),
        vec![ConfigAdvisory::WebuiPublicReadOnlyOffLoopback {
            listen: "0.0.0.0:9843".to_owned()
        }]
    );

    // A disabled UI advises nothing, whatever else is set.
    config.webui.enabled = false;
    config.webui.authdb.enabled = false;
    assert!(config.advisories().is_empty());
}

#[test]
fn only_public_read_only_is_reload_applied_in_the_webui_table() {
    let old = valid_config();
    let mut new = valid_config();
    new.webui.public_read_only = true;
    assert!(restart_only_differences(&old, &new).is_empty());

    new.webui.enabled = false;
    new.webui.session_ttl_seconds = 3_600;
    new.webui.authdb.enabled = false;
    new.webui.oidc.client_id = Some("changed".to_owned());
    assert_eq!(
        restart_only_differences(&old, &new),
        vec![
            "webui.enabled",
            "webui.session_ttl_seconds",
            "webui.authdb.enabled",
            "webui.oidc",
        ]
    );
}

#[test]
fn non_normalized_absolute_config_paths_are_rejected() {
    for path in [
        "/private/var/./flanforge.log",
        "/private/var//flanforge.log",
        "/private/var/../flanforge.log",
    ] {
        let mut config = valid_config();
        config.logging.path = Some(path.into());
        assert_eq!(
            config.ensure_valid(),
            Err(ConfigError::InvalidPath {
                field: "logging.path"
            }),
            "accepted {path}"
        );
    }
}

#[test]
fn libvirt_pool_and_network_reject_consecutive_dots() {
    for field in ["pool", "network"] {
        let mut config = valid_config();
        config.runtime.backend = RuntimeBackendConfig::Libvirt(LibvirtConfig {
            image_manifest_dir: "/private/libvirt/published-bases".into(),
            ..LibvirtConfig::default()
        });
        config.runtime.max_running_vms = 1;
        for profile in config.profiles.values_mut() {
            profile.network = NetworkMode::Default;
        }
        let backend = config
            .runtime
            .libvirt_mut()
            .unwrap_or_else(|| unreachable!("libvirt fixture"));
        match field {
            "pool" => backend.pool = "ci..pool".to_owned(),
            "network" => backend.network = "ci..network".to_owned(),
            _ => unreachable!("field fixture"),
        }
        let error_field = match field {
            "pool" => "runtime.backend.pool",
            "network" => "runtime.backend.network",
            _ => unreachable!("field fixture"),
        };
        assert_eq!(
            config.ensure_valid(),
            Err(ConfigError::UnsafeValue { field: error_field })
        );
    }
}

#[test]
fn quoted_known_hosts_path_is_rejected() {
    for path in ["/private/known\"_hosts", "/private/known\\_hosts"] {
        let mut config = valid_config();
        ssh_mut(&mut config).known_hosts_file = Some(path.into());
        assert_eq!(
            config.ensure_valid(),
            Err(ConfigError::InvalidPath {
                field: "guest.ssh.known_hosts_file"
            }),
            "accepted {path}"
        );
    }
}

#[test]
fn host_key_verification_defaults_to_enabled_and_requires_its_anchor() {
    let mut config = valid_config();
    assert!(ssh(&config).verify_host_key);
    assert_eq!(config.ensure_valid(), Ok(()));

    ssh_mut(&mut config).host_key_alias = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingGuestSetting {
            field: "host_key_alias"
        })
    );
    ssh_mut(&mut config).host_key_alias = Some("tart-ci".into());
    ssh_mut(&mut config).known_hosts_file = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingGuestSetting {
            field: "known_hosts_file"
        })
    );
}

#[test]
fn disabled_host_key_verification_does_not_require_or_validate_the_anchor() {
    let mut config = valid_config();
    let ssh = ssh_mut(&mut config);
    ssh.verify_host_key = false;
    ssh.known_hosts_file = None;
    ssh.host_key_alias = None;
    assert_eq!(config.ensure_valid(), Ok(()));

    ssh_mut(&mut config).known_hosts_file = Some("relative/known\"_hosts".into());
    assert_eq!(config.ensure_valid(), Ok(()));
}

/// Binding is a deployment choice: a container needs its pod address, and the
/// OIDC check on every allocation request is what authorizes callers.
#[test]
fn any_listen_address_the_operator_configures_is_accepted() {
    for listen in [
        "127.0.0.1:9843",
        "0.0.0.0:9843",
        "10.42.0.7:9843",
        "100.101.102.103:9843",
        "[::]:9843",
    ] {
        let mut config = valid_config();
        config.server.listen = listen
            .parse()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(config.ensure_valid(), Ok(()), "rejected {listen}");
    }
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

// The label alone: distinct repositories, so nothing but the shared label can
// be what rejects this.
#[test]
fn duplicate_runner_label_is_rejected() {
    let mut config = valid_config();
    let profile = config
        .profiles
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| unreachable!("fixture profile"));
    let mut second = profile.clone();
    second.repository =
        crate::RepositoryName::new("owner/liftfg").unwrap_or_else(|error| unreachable!("{error}"));
    config.profiles = BTreeMap::from([
        (profile_name("one"), profile),
        (profile_name("two"), second),
    ]);
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::DuplicateRunnerLabel)
    );
}

// One repository, two Apple policies: a protected-tag release and a cheap
// pull-request check. Each keeps its own rules through the round trip.
#[test]
fn profiles_may_share_a_repository_with_distinct_rules() {
    let release = profile_from(
        r#"
        repository = "owner/halogen"
        template = "flanforge-base"
        runner_label = "macos-tart-release"
        job_name = "apple-release"
        allowed_workflows = ["release.yml"]
        allowed_events = ["push"]
        allowed_refs = ["refs/tags/v*"]
        require_protected_ref = true
        cpu_count = 8
        memory_mb = 16384
        boot_timeout_seconds = 300
        idle_timeout_seconds = 600
        job_timeout_seconds = 10800
        cleanup_timeout_seconds = 120
    "#,
    );
    let check = profile_from(
        r#"
        repository = "owner/halogen"
        template = "flanforge-base"
        runner_label = "macos-tart-check"
        job_name = "apple-check"
        allowed_workflows = ["pull-request.yml"]
        allowed_events = ["pull_request"]
        allowed_refs = ["refs/heads/*"]
        require_protected_ref = false
        cpu_count = 4
        memory_mb = 8192
        boot_timeout_seconds = 300
        idle_timeout_seconds = 600
        job_timeout_seconds = 1800
        cleanup_timeout_seconds = 120
    "#,
    );
    let mut config = valid_config();
    config.profiles = BTreeMap::from([
        (profile_name("release"), release),
        (profile_name("check"), check),
    ]);
    assert_eq!(config.ensure_valid(), Ok(()));

    let release = &config.profiles[&profile_name("release")];
    let check = &config.profiles[&profile_name("check")];
    assert_eq!(release.repository, check.repository);
    assert_ne!(release.runner_label, check.runner_label);
    assert_eq!(
        release.allowed_workflows,
        BTreeSet::from(["release.yml".to_owned()])
    );
    assert_eq!(
        check.allowed_workflows,
        BTreeSet::from(["pull-request.yml".to_owned()])
    );
    assert_eq!(
        release.allowed_refs,
        BTreeSet::from(["refs/tags/v*".to_owned()])
    );
    assert_eq!(
        check.allowed_refs,
        BTreeSet::from(["refs/heads/*".to_owned()])
    );
    assert!(release.require_protected_ref);
    assert!(!check.require_protected_ref);
    assert_eq!(
        (
            release.cpu_count,
            release.memory_mb,
            release.job_timeout_seconds
        ),
        (8, 16_384, 10_800)
    );
    assert_eq!(
        (check.cpu_count, check.memory_mb, check.job_timeout_seconds),
        (4, 8_192, 1_800)
    );
}

// Profile names stay unique because profiles are a map, and a repeated table
// key never reaches validation.
#[test]
fn duplicate_profile_names_are_rejected_by_the_document() {
    let source = r#"
        [profiles.halogen]
        repository = "owner/halogen"

        [profiles.halogen]
        repository = "owner/liftfg"
    "#;
    assert!(toml::from_str::<toml::Value>(source).is_err());
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
        "--accept-dns=false --advertise-tags='tag:ci,tag:apple builder'".into();
    assert_eq!(config.ensure_valid(), Ok(()));
    assert_eq!(
        config
            .tailscale
            .extra_arguments()
            .unwrap_or_else(|error| unreachable!("parse: {error}")),
        [
            "--accept-dns=false",
            "--advertise-tags=tag:ci,tag:apple builder",
        ]
    );
}

#[test]
fn tailscale_extra_arguments_reject_daemon_owned_flags() {
    for name in [
        "auth-key",
        "authkey",
        "login-server",
        "operator",
        "hostname",
    ] {
        for arguments in [format!("--{name}=override"), format!("--{name} override")] {
            let mut config = valid_config();
            config.tailscale.extra_args = arguments.clone();
            assert_eq!(
                config.ensure_valid(),
                Err(ConfigError::InvalidTailscaleArguments),
                "accepted {arguments}"
            );
        }
    }
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
    config
        .runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
        .home = Some("/Users/runner/.tart".into());
    let profile = profile_mut(&mut config);
    profile.warm_template = crate::VmName::new("halogen-warm").ok();
    profile.regeneration_workflow = Some("apple.yml".to_owned());
    config
}

#[test]
fn existing_config_loads_without_the_new_fields() {
    let config = valid_config();
    assert!(matches!(
        config.runtime.backend,
        RuntimeBackendConfig::Tart(_)
    ));
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
fn legacy_tart_fields_serialize_only_as_the_canonical_backend() {
    let runtime: RuntimeConfig = toml::from_str(
        r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        tart_path = "/usr/local/bin/tart"
        tart_home = "/tmp/tart"
        forgejo_runner_host_path = "/tmp/forgejo-runner"
        "#,
    )
    .unwrap_or_else(|error| unreachable!("legacy fixture: {error}"));
    let encoded = toml::to_string(&runtime)
        .unwrap_or_else(|error| unreachable!("canonical encoding: {error}"));
    assert!(encoded.contains("[backend]"));
    assert!(encoded.contains("kind = \"tart\""));
    assert!(encoded.contains("path = \"/usr/local/bin/tart\""));
    assert!(encoded.contains("home = \"/tmp/tart\""));
    assert!(encoded.contains("runner_host_path = \"/tmp/forgejo-runner\""));
    assert!(!encoded.contains("tart_path"));
    assert!(!encoded.contains("tart_home"));
    assert!(!encoded.contains("forgejo_runner_host_path"));
}

#[test]
fn canonical_backend_rejects_legacy_and_cross_backend_fields() {
    let mixed = r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        tart_path = "/usr/local/bin/tart"
        [backend]
        kind = "tart"
    "#;
    assert!(toml::from_str::<RuntimeConfig>(mixed).is_err());

    let libvirt_with_tart = r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        [backend]
        kind = "libvirt"
        uri = "qemu:///system"
        pool = "flanforge"
        network = "flanforge-ci"
        image_manifest_dir = "/var/lib/flanforge/images"
        path = "/usr/local/bin/tart"
    "#;
    assert!(toml::from_str::<RuntimeConfig>(libvirt_with_tart).is_err());

    let tart_with_libvirt = r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        [backend]
        kind = "tart"
        uri = "qemu:///system"
    "#;
    assert!(toml::from_str::<RuntimeConfig>(tart_with_libvirt).is_err());
}

fn libvirt_config() -> Config {
    let mut config = valid_config();
    config.runtime.backend = RuntimeBackendConfig::Libvirt(LibvirtConfig {
        uri: "qemu:///system".to_owned(),
        allow_insecure_transport: false,
        pool: "flanforge".to_owned(),
        network: "flanforge-ci".to_owned(),
        image_manifest_dir: "/var/lib/flanforge/images".into(),
        qemu_img_path: "/usr/bin/qemu-img".into(),
        virsh_path: "/usr/bin/virsh".into(),
        min_storage_free_mb: 32_768,
        warm_capture_timeout_seconds: 1_800,
        image_import_dir: None,
        image_reimport: crate::ImageReimport::Refuse,
    });
    config.runtime.max_running_vms = 1;
    config.runtime.host_cpu_count = Some(16);
    config.runtime.host_memory_mb = Some(65_536);
    config.runtime.host_storage_mb = Some(262_144);
    profile_mut(&mut config).network = NetworkMode::Default;
    let ssh = ssh_mut(&mut config);
    ssh.known_hosts_file = None;
    ssh.host_key_alias = None;
    config.guest.runner_user = super::LIBVIRT_GUEST_USER.to_owned();
    config
}

#[test]
fn tagged_libvirt_backend_is_strict_and_validated() {
    let config = libvirt_config();
    assert_eq!(config.runtime.backend_kind(), RuntimeBackendKind::Libvirt);
    assert_eq!(config.ensure_valid(), Ok(()));

    let source = toml::to_string(&config).unwrap_or_else(|error| unreachable!("{error}"));
    let decoded: Config = toml::from_str(&source).unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(decoded.runtime.backend, config.runtime.backend);

    // A remote host is a supported topology, so an authenticated transport is
    // admitted; clear-text tcp needs the operator to say so first.
    for (uri, allow_insecure, expected) in [
        ("qemu+ssh://host/system", false, Ok(())),
        ("qemu+tls://host.example:16514/system", false, Ok(())),
        (
            "qemu+tcp://host/system",
            false,
            Err(ConfigError::UnsafeValue {
                field: "runtime.backend.uri",
            }),
        ),
        ("qemu+tcp://host/system", true, Ok(())),
        (
            "qemu+tls://host/system?no_verify=1",
            true,
            Err(ConfigError::UnsafeValue {
                field: "runtime.backend.uri",
            }),
        ),
    ] {
        let mut remote = libvirt_config();
        let RuntimeBackendConfig::Libvirt(libvirt) = &mut remote.runtime.backend else {
            unreachable!("libvirt fixture")
        };
        libvirt.uri = uri.to_owned();
        libvirt.allow_insecure_transport = allow_insecure;
        assert_eq!(
            remote.ensure_valid(),
            expected,
            "{uri} (insecure: {allow_insecure})"
        );
    }
}

#[test]
fn libvirt_profile_must_fit_beside_the_storage_reserve() {
    let mut config = libvirt_config();
    let profile = profile_mut(&mut config);
    profile.storage_mb = 240_000;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));

    profile_mut(&mut config).storage_mb = 229_376;
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn libvirt_cleanup_timeout_reserves_parent_helper_slack() {
    let mut config = libvirt_config();
    profile_mut(&mut config).cleanup_timeout_seconds = 5;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
    profile_mut(&mut config).cleanup_timeout_seconds = 6;
    assert_eq!(config.ensure_valid(), Ok(()));

    let mut tart = valid_config();
    profile_mut(&mut tart).cleanup_timeout_seconds = 5;
    assert_eq!(tart.ensure_valid(), Ok(()));
}

#[test]
fn libvirt_requires_per_allocation_host_key_pinning() {
    let mut disabled = libvirt_config();
    ssh_mut(&mut disabled).verify_host_key = false;
    assert_eq!(
        disabled.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "guest.ssh.verify_host_key"
        })
    );

    let mut inherited = libvirt_config();
    ssh_mut(&mut inherited).host_key_alias = Some("tart-ci".to_owned());
    assert_eq!(
        inherited.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "guest allocation host-key anchor"
        })
    );
}

#[test]
fn libvirt_guest_account_is_the_operators_and_the_anchor_is_not() {
    // The base image provides the account, so its name is a deployment choice.
    let mut config = libvirt_config();
    config.guest.runner_user = "builder".to_owned();
    assert_eq!(config.ensure_valid(), Ok(()));

    // The per-allocation host key is generated fresh and pinned, so a
    // long-lived anchor would contradict it.
    let mut anchored = libvirt_config();
    ssh_mut(&mut anchored).host_key_alias = Some("libvirt-ci".into());
    assert_eq!(
        anchored.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "guest allocation host-key anchor"
        })
    );

    let mut unverified = libvirt_config();
    ssh_mut(&mut unverified).verify_host_key = false;
    assert_eq!(
        unverified.ensure_valid(),
        Err(ConfigError::UnsafeValue {
            field: "guest.ssh.verify_host_key"
        })
    );
}

#[test]
fn backend_specific_slot_defaults_preserve_tart_and_serialize_libvirt() {
    let native: RuntimeConfig = toml::from_str(
        r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        "#,
    )
    .unwrap_or_else(|error| unreachable!("{error}"));
    #[cfg(target_os = "macos")]
    {
        assert_eq!(native.backend_kind(), RuntimeBackendKind::Tart);
        assert_eq!(native.max_running_vms, 2);
    }
    #[cfg(target_os = "linux")]
    {
        assert_eq!(native.backend_kind(), RuntimeBackendKind::Libvirt);
        assert_eq!(native.max_running_vms, 1);
    }

    let tart: RuntimeConfig = toml::from_str(
        r#"
        state_dir = "/tmp/state"
        vm_prefix = "ci-"
        tart_path = "/usr/bin/tart"
        "#,
    )
    .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(tart.backend_kind(), RuntimeBackendKind::Tart);
    assert_eq!(tart.max_running_vms, 2);

    let libvirt: RuntimeConfig = toml::from_str(
        r#"
        state_dir = "/var/lib/flanforge"
        vm_prefix = "ci-"

        [backend]
        kind = "libvirt"
        uri = "qemu:///system"
        pool = "flanforge"
        network = "flanforge-ci"
        "#,
    )
    .unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(libvirt.max_running_vms, 1);
    assert_eq!(
        libvirt
            .libvirt()
            .map(|backend| backend.image_manifest_dir.as_path()),
        Some(std::path::Path::new(
            "/var/lib/flanforge/libvirt/published-bases"
        ))
    );
}

#[test]
fn libvirt_rejects_tart_only_profile_capabilities() {
    let mut softnet = libvirt_config();
    profile_mut(&mut softnet).network = NetworkMode::Softnet;
    assert!(matches!(
        softnet.ensure_valid(),
        Err(ConfigError::UnsupportedBackendCapability {
            backend: "libvirt",
            capability: "softnet networking",
            ..
        })
    ));
}

/// Warm images are supported on libvirt, and retirement lives in the sweep, so
/// a disabled sweep is refused rather than silently pinning generations.
#[test]
fn libvirt_warm_requires_a_sweep_interval() {
    let mut warm = libvirt_config();
    let profile = profile_mut(&mut warm);
    profile.warm_template = crate::VmName::new("halogen-warm").ok();
    profile.regeneration_workflow = Some("apple.yml".to_owned());
    warm.runtime.reap_interval_hours = 168;
    assert_eq!(warm.ensure_valid(), Ok(()));

    warm.runtime.reap_interval_hours = 0;
    assert_eq!(
        warm.ensure_valid(),
        Err(ConfigError::MissingRuntimeSetting {
            field: "reap_interval_hours"
        })
    );
}

/// The capture budget is a qcow2-convert budget, not a clone budget.
#[test]
fn libvirt_warm_capture_timeout_is_bounded() {
    let mut config = libvirt_config();
    for (seconds, is_valid) in [(59_u64, false), (60, true), (7_200, true), (7_201, false)] {
        config
            .runtime
            .libvirt_mut()
            .unwrap_or_else(|| unreachable!("libvirt fixture"))
            .warm_capture_timeout_seconds = seconds;
        assert_eq!(config.ensure_valid().is_ok(), is_valid, "{seconds}s");
    }
}

#[test]
fn libvirt_accepts_backend_specific_tailscale_bootstrap() {
    let mut config = libvirt_config();
    config.tailscale.enabled = true;
    config.tailscale.preauth_key_file = Some("/etc/flanforge/tailscale-key".into());
    config.tailscale.login_server = Url::parse("https://headscale.example").ok();
    assert_eq!(config.ensure_valid(), Ok(()));
}

#[test]
fn libvirt_publications_cannot_overlap_authoritative_state_namespaces() {
    for path in [
        "/tmp/flanforge/images",
        "/tmp/flanforge/libvirt/allocations",
        "/tmp/flanforge/libvirt/imports/nested",
    ] {
        let mut config = libvirt_config();
        config.runtime.state_dir = "/tmp/flanforge".into();
        config
            .runtime
            .libvirt_mut()
            .unwrap_or_else(|| unreachable!("libvirt"))
            .image_manifest_dir = path.into();
        assert_eq!(
            config.ensure_valid(),
            Err(ConfigError::UnsafeValue {
                field: "runtime.backend.image_manifest_dir"
            }),
            "{path}"
        );
    }
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

    // Satisfied by a pattern rather than by appearing verbatim.
    let mut config = warm_config();
    let profile = profile_mut(&mut config);
    profile.allowed_workflows.insert("*.yml".to_owned());
    profile.regeneration_workflow = Some("warm.yml".to_owned());
    assert_eq!(config.ensure_valid(), Ok(()));

    // But it names one real file, so it may not itself be a pattern.
    let mut config = warm_config();
    let profile = profile_mut(&mut config);
    profile.allowed_workflows.insert("*.yml".to_owned());
    profile.regeneration_workflow = Some("*.yml".to_owned());
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile { .. })
    ));
}

// The only fail-closed guard against a profile that carries no authorization
// policy at all, and so would authorize every workflow, event, and ref for its
// repository.
#[track_caller]
fn assert_empty_allowlist_rejected(case: &str, empty: impl FnOnce(&mut Profile)) {
    let mut config = valid_config();
    empty(profile_mut(&mut config));
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::InvalidProfile {
            profile: "halogen".to_owned(),
            message: "authorization allowlists cannot be empty".to_owned(),
        }),
        "accepted an empty {case}"
    );
}

#[test]
fn empty_authorization_allowlists_are_rejected() {
    assert_empty_allowlist_rejected("allowed_workflows", |profile| {
        profile.allowed_workflows.clear();
    });
    assert_empty_allowlist_rejected("allowed_events", |profile| profile.allowed_events.clear());
    assert_empty_allowlist_rejected("allowed_refs", |profile| profile.allowed_refs.clear());
}

#[test]
fn allowlists_accept_glob_patterns() {
    let mut config = valid_config();
    let profile = profile_mut(&mut config);
    profile.allowed_workflows = BTreeSet::from(["*.yml".to_owned()]);
    profile.allowed_refs = BTreeSet::from(["refs/heads/*".to_owned()]);
    assert_eq!(config.ensure_valid(), Ok(()));

    // Events are literal, so a wildcard is refused at load rather than
    // silently matching triggers nobody named.
    for pattern in ["workflow_*", "*"] {
        let mut config = valid_config();
        profile_mut(&mut config).allowed_events = BTreeSet::from([pattern.to_owned()]);
        assert!(
            matches!(
                config.ensure_valid(),
                Err(ConfigError::InvalidProfile { .. })
            ),
            "accepted event pattern {pattern}"
        );
    }

    // A ref pattern still has to name a namespace and stay command-safe.
    for unsafe_pattern in ["*", "refs/heads/*;touch"] {
        let mut config = valid_config();
        profile_mut(&mut config).allowed_refs = BTreeSet::from([unsafe_pattern.to_owned()]);
        assert!(
            matches!(
                config.ensure_valid(),
                Err(ConfigError::InvalidProfile { .. })
            ),
            "accepted {unsafe_pattern}"
        );
    }
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
    config
        .runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
        .home = None;
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::MissingRuntimeSetting {
            field: "backend.home"
        })
    );

    let mut config = valid_config();
    config
        .runtime
        .tart_mut()
        .unwrap_or_else(|| unreachable!("Tart fixture"))
        .home = None;
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
    new.runtime.backend = RuntimeBackendConfig::Libvirt(LibvirtConfig {
        uri: "qemu:///system".to_owned(),
        allow_insecure_transport: false,
        pool: "flanforge".to_owned(),
        network: "flanforge-ci".to_owned(),
        image_manifest_dir: "/var/lib/flanforge/images".into(),
        qemu_img_path: "/usr/bin/qemu-img".into(),
        virsh_path: "/usr/bin/virsh".into(),
        min_storage_free_mb: 32_768,
        warm_capture_timeout_seconds: 1_800,
        image_import_dir: None,
        image_reimport: crate::ImageReimport::Refuse,
    });
    new.server.listen = "127.0.0.1:9999"
        .parse()
        .unwrap_or_else(|error| unreachable!("{error}"));
    new.forgejo.api_token_file = "/private/token".into();
    new.db.db_path = Some("/private/flanforge.db".into());
    assert_eq!(
        restart_only_differences(&old, &new),
        vec![
            "server.listen",
            "server.allocation_wait_seconds",
            "db.db_path",
            "forgejo.api_token_file",
            "runtime.state_dir",
            "runtime.backend",
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

/// A document whose `[runtime.backend]` and `[guest]` tables are the variables,
/// so channel resolution can be exercised without restating a whole file.
fn channel_source(backend: &str, guest: &str) -> String {
    format!(
        r#"
        [server]
        listen = "127.0.0.1:9843"

        [oidc]
        issuer = "https://forgejo.example/api/actions"
        audience = "flanforged"
        jwks_url = "https://forgejo.example/api/actions/.well-known/jwks"

        [forgejo]
        api_url = "https://forgejo.example/api/v1/"
        api_token_file = "/etc/flanforge/token"

        [runtime]
        state_dir = "/var/lib/flanforge"
        vm_prefix = "ci-"
        max_running_vms = 1
        host_cpu_count = 16
        host_memory_mb = 65536
        host_storage_mb = 262144

        [runtime.backend]
        {backend}

        [guest]
        {guest}

        [profiles.halogen]
        repository = "owner/halogen"
        template = "flanforge-base"
        runner_label = "linux-halogen"
        job_name = "build"
        allowed_workflows = ["ci.yml"]
        allowed_events = ["push"]
        allowed_refs = ["refs/heads/main"]
        network = "default"
        cpu_count = 4
        memory_mb = 8192
        boot_timeout_seconds = 300
        idle_timeout_seconds = 600
        job_timeout_seconds = 7200
        cleanup_timeout_seconds = 120
    "#
    )
}

const LIBVIRT_BACKEND: &str = r#"kind = "libvirt"
        uri = "qemu:///system"
        pool = "flanforge"
        network = "flanforge-ci"
        image_manifest_dir = "/var/lib/flanforge/libvirt/published-bases""#;

const TART_BACKEND: &str = r#"kind = "tart"
        path = "/opt/homebrew/bin/tart"
        runner_host_path = "/opt/flanforge/forgejo-runner""#;

const AGENT_GUEST: &str = r#"runner_user = "runner"
        forgejo_runner_path = "/usr/local/bin/forgejo-runner""#;

fn channel_config(backend: &str, guest: &str) -> Config {
    toml::from_str(&channel_source(backend, guest)).unwrap_or_else(|error| unreachable!("{error}"))
}

/// The default is the backend's, resolved eagerly against the same document.
#[test]
fn an_omitted_channel_resolves_from_the_backend_it_is_written_beside() {
    let libvirt = channel_config(LIBVIRT_BACKEND, AGENT_GUEST);
    assert_eq!(libvirt.guest.channel, GuestChannelKind::Agent);
    assert!(libvirt.guest.ssh.is_none());
    assert_eq!(libvirt.ensure_valid(), Ok(()));

    let tart = channel_config(
        TART_BACKEND,
        &format!(
            "{AGENT_GUEST}\n\n        [guest.ssh]\n        identity_file = \"/private/id\"\n        \
             known_hosts_file = \"/private/known_hosts\"\n        host_key_alias = \"tart-ci\""
        ),
    );
    assert_eq!(tart.guest.channel, GuestChannelKind::Ssh);
    assert_eq!(tart.ensure_valid(), Ok(()));
}

/// The administrator already chose to carry libvirt's API in clear text; a new
/// default must not also put the job's credentials on that wire unasked.
#[test]
fn a_clear_text_libvirt_transport_resolves_to_ssh_and_refuses_an_explicit_agent() {
    let backend = r#"kind = "libvirt"
        uri = "qemu+tcp://virt.example/system"
        allow_insecure_transport = true
        pool = "flanforge"
        network = "flanforge-ci"
        image_manifest_dir = "/var/lib/flanforge/libvirt/published-bases""#;
    let resolved = channel_config(
        backend,
        &format!(
            "{AGENT_GUEST}\n\n        [guest.ssh]\n        identity_file = \"/etc/flanforge/id\""
        ),
    );
    assert_eq!(resolved.guest.channel, GuestChannelKind::Ssh);
    assert_eq!(resolved.ensure_valid(), Ok(()));

    let explicit = channel_config(
        backend,
        &format!("channel = \"agent\"\n        {AGENT_GUEST}"),
    );
    assert_eq!(
        explicit.ensure_valid(),
        Err(ConfigError::InsecureGuestChannelTransport)
    );
}

#[test]
fn tart_has_no_guest_agent_to_select() {
    let config = channel_config(
        TART_BACKEND,
        &format!(
            "channel = \"agent\"\n        {AGENT_GUEST}\n\n        [guest.ssh]\n        \
             identity_file = \"/private/id\"\n        known_hosts_file = \
             \"/private/known_hosts\"\n        host_key_alias = \"tart-ci\""
        ),
    );
    assert_eq!(
        config.ensure_valid(),
        Err(ConfigError::UnsupportedBackendCapability {
            backend: "tart",
            capability: "the QEMU guest agent channel",
            profile: None,
        })
    );
}

/// The same file loads on one host and fails on another, so the error names
/// the resolution that made the table required.
#[test]
fn the_ssh_channel_names_the_backend_and_channel_that_require_its_table() {
    let config = channel_config(
        LIBVIRT_BACKEND,
        &format!("channel = \"ssh\"\n        {AGENT_GUEST}"),
    );
    let error = ConfigError::MissingGuestTable {
        table: "guest.ssh",
        backend: RuntimeBackendKind::Libvirt,
        channel: GuestChannelKind::Ssh,
    };
    assert_eq!(config.ensure_valid(), Err(error.clone()));
    assert_eq!(
        error.to_string(),
        "guest.ssh is required because guest.channel resolved to \"ssh\" for the libvirt backend"
    );
}

/// Selecting the agent channel must never be a way to smuggle a weak SSH
/// configuration past validation.
#[test]
fn the_agent_channel_does_not_weaken_a_present_ssh_table() {
    for (setting, expected) in [
        (
            "identity_file = \"etc/flanforge/id\"",
            ConfigError::InvalidPath {
                field: "guest.ssh.identity_file",
            },
        ),
        (
            "identity_file = \"/etc/../flanforge/id\"",
            ConfigError::InvalidPath {
                field: "guest.ssh.identity_file",
            },
        ),
        (
            "identity_file = \"/etc/flanforge/id\"\n        connect_timeout_seconds = 0",
            ConfigError::OutOfRange {
                field: "guest.ssh.connect_timeout_seconds",
            },
        ),
        (
            "identity_file = \"/etc/flanforge/id\"\n        connect_timeout_seconds = 61",
            ConfigError::OutOfRange {
                field: "guest.ssh.connect_timeout_seconds",
            },
        ),
        (
            "identity_file = \"/etc/flanforge/id\"\n        verify_host_key = false",
            ConfigError::UnsafeValue {
                field: "guest.ssh.verify_host_key",
            },
        ),
        (
            "identity_file = \"/etc/flanforge/id\"\n        known_hosts_file = \
             \"/etc/flanforge/known_hosts\"",
            ConfigError::UnsafeValue {
                field: "guest allocation host-key anchor",
            },
        ),
    ] {
        let config = channel_config(
            LIBVIRT_BACKEND,
            &format!(
                "channel = \"agent\"\n        {AGENT_GUEST}\n\n        [guest.ssh]\n        \
                 {setting}"
            ),
        );
        assert_eq!(config.ensure_valid(), Err(expected), "accepted {setting}");
    }
}

#[test]
fn a_changed_channel_or_ssh_table_is_reported_as_restart_only() {
    let agent = channel_config(LIBVIRT_BACKEND, AGENT_GUEST);
    let with_ssh = channel_config(
        LIBVIRT_BACKEND,
        &format!(
            "{AGENT_GUEST}\n\n        [guest.ssh]\n        identity_file = \"/etc/flanforge/id\""
        ),
    );
    assert_eq!(
        restart_only_differences(&agent, &with_ssh),
        vec!["guest.ssh"]
    );

    let mut widened = with_ssh.clone();
    ssh_mut(&mut widened).connect_timeout_seconds = 9;
    widened.guest.runner_user = "builder".to_owned();
    assert_eq!(
        restart_only_differences(&with_ssh, &widened),
        vec!["guest.runner_user", "guest.ssh.connect_timeout_seconds"]
    );

    let mut over_ssh = agent.clone();
    over_ssh.guest.channel = GuestChannelKind::Ssh;
    assert_eq!(
        restart_only_differences(&agent, &over_ssh),
        vec!["guest.channel"]
    );
}

fn hot_config() -> Config {
    let mut config = valid_config();
    config.runtime.max_hot_vms = 1;
    profile_mut(&mut config).hot = Some(HotConfig {
        enabled: true,
        ..HotConfig::default()
    });
    config
}

fn enabled_hot() -> HotConfig {
    HotConfig {
        enabled: true,
        ..HotConfig::default()
    }
}

fn hot_mut(config: &mut Config) -> &mut HotConfig {
    profile_mut(config)
        .hot
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture hot table"))
}

#[test]
fn a_hot_pool_larger_than_the_host_slot_count_is_refused() {
    let mut config = hot_config();
    assert_eq!(config.ensure_valid(), Ok(()));

    config.runtime.max_hot_vms = config.runtime.max_running_vms + 1;
    assert!(matches!(
        config.ensure_valid(),
        Err(ConfigError::OutOfRange {
            field: "runtime.max_hot_vms"
        })
    ));
}

#[test]
fn an_enabled_hot_bound_outside_its_range_is_refused() {
    for out_of_range in [
        HotConfig {
            reset_timeout_seconds: 601,
            ..enabled_hot()
        },
        HotConfig {
            max_jobs: 1_001,
            ..enabled_hot()
        },
        HotConfig {
            max_lifetime_seconds: 299,
            ..enabled_hot()
        },
        HotConfig {
            idle_ttl_seconds: 29,
            ..enabled_hot()
        },
    ] {
        let mut config = hot_config();
        *hot_mut(&mut config) = out_of_range;
        assert!(
            matches!(
                config.ensure_valid(),
                Err(ConfigError::InvalidProfile { .. })
            ),
            "{out_of_range:?}"
        );
    }
}

/// A disabled table bounds nothing the daemon acts on, so an out-of-range knob
/// inside it must not block the load that switches hot off.
#[test]
fn a_disabled_hot_table_is_not_bounds_checked() {
    let mut config = hot_config();
    *hot_mut(&mut config) = HotConfig {
        enabled: false,
        max_jobs: 1_001,
        idle_ttl_seconds: 29,
        ..HotConfig::default()
    };

    assert!(config.ensure_valid().is_ok());
}

// The project's rule on insecure settings: a risky lane is the operator's to
// choose, so it is named at load and never refused.
#[test]
fn a_lane_that_admits_a_fork_event_warns_and_still_loads() {
    let mut config = hot_config();
    hot_mut(&mut config).lanes = HotLanePolicy::Any;
    profile_mut(&mut config)
        .allowed_events
        .insert("pull_request".to_owned());

    assert_eq!(config.ensure_valid(), Ok(()));
    assert!(
        config
            .advisories()
            .contains(&ConfigAdvisory::HotLaneAdmitsForkEvent {
                profile: "halogen".to_owned(),
                event: "pull_request".to_owned(),
            })
    );
}

// Every one of these was a refusal in the first draft. Each would have made a
// documented or emergency configuration unloadable, so each is an advisory.
#[test]
fn an_unsatisfiable_but_deliberate_hot_setting_warns_and_still_loads() {
    for (mutate, expected) in [
        (
            (|config: &mut Config| config.runtime.max_hot_vms = 0) as fn(&mut Config),
            ConfigAdvisory::HotDisabledByGlobalCap {
                profile: "halogen".to_owned(),
            },
        ),
        (
            |config: &mut Config| hot_mut(config).max_idle = 0,
            ConfigAdvisory::HotIdlePoolEmpty {
                profile: "halogen".to_owned(),
            },
        ),
        // The same "configured to do nothing" mistake as max_idle = 0, so it
        // gets the same treatment rather than a refusal.
        (
            |config: &mut Config| hot_mut(config).max_jobs = 0,
            ConfigAdvisory::HotJobBudgetEmpty {
                profile: "halogen".to_owned(),
            },
        ),
        (
            |config: &mut Config| hot_mut(config).max_idle = 4,
            ConfigAdvisory::HotIdleExceedsGlobalCap {
                profile: "halogen".to_owned(),
                max_idle: 4,
                max_hot_vms: 1,
            },
        ),
        (
            |config: &mut Config| hot_mut(config).max_lifetime_seconds = 300,
            ConfigAdvisory::HotLifetimeBelowJobTimeout {
                profile: "halogen".to_owned(),
                max_lifetime_seconds: 300,
                job_timeout_seconds: 7_200,
            },
        ),
        (
            |config: &mut Config| hot_mut(config).lanes = HotLanePolicy::None,
            ConfigAdvisory::HotLaneAdmitsNothing {
                profile: "halogen".to_owned(),
            },
        ),
        (
            |config: &mut Config| hot_mut(config).simulator_reset = SimulatorReset::None,
            ConfigAdvisory::HotSimulatorStateRetained {
                profile: "halogen".to_owned(),
            },
        ),
    ] {
        let mut config = hot_config();
        mutate(&mut config);
        assert_eq!(config.ensure_valid(), Ok(()), "{expected:?}");
        assert!(config.advisories().contains(&expected), "{expected:?}");
    }
}

#[test]
fn a_profile_without_a_hot_table_is_advised_of_nothing() {
    let config = valid_config();
    assert_eq!(config.ensure_valid(), Ok(()));
    assert!(config.advisories().is_empty());
    assert!(profile_mut(&mut valid_config()).hot.is_none());

    // A well-formed hot table warns about nothing: every advisory names a
    // choice, and the recommended shape makes none of them.
    assert_eq!(hot_config().advisories(), Vec::new());
}

/// The permitted range is a sanity bound rather than a policy, and the same for
/// both backends: what really bounds concurrent guests is the host budget,
/// which admission folds per allocation. Only the *defaults* differ.
#[test]
fn max_running_vms_is_bounded_by_the_host_budget_rather_than_by_the_backend() {
    for mut config in [valid_config(), libvirt_config()] {
        for count in [1, 2, 4, u8::MAX] {
            config.runtime.max_running_vms = count;
            assert_eq!(
                config.ensure_valid(),
                Ok(()),
                "max_running_vms = {count} was refused"
            );
        }
        // Zero admits nothing at all, so it stays a refusal.
        config.runtime.max_running_vms = 0;
        assert!(matches!(
            config.ensure_valid(),
            Err(ConfigError::OutOfRange {
                field: "runtime.max_running_vms",
                ..
            })
        ));
    }
}
