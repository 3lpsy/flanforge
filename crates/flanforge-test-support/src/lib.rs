mod capture;
mod script;

pub use capture::capture_logs;
pub use script::executable;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use flanforge_core::{
    AllocationRequest, Config, ForgejoClaims, GuestSize, HotConfig, HotLanePolicy, NetworkMode,
    OidcConfig, Profile, ProfileName, RepositoryName, RunnerLabel, ServerConfig, VmName, VmPrefix,
};

#[must_use]
pub fn profile_name() -> ProfileName {
    ProfileName::new("project").unwrap_or_else(|error| unreachable!("fixture: {error}"))
}
use url::Url;

#[must_use]
pub fn profile() -> Profile {
    Profile {
        repository: RepositoryName::new("owner/project")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        template: VmName::new("flanforge-base")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        runner_label: RunnerLabel::new("macos-tart-project")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        job_name: "apple-build".into(),
        allowed_workflows: ["apple.yml".to_owned()].into(),
        allowed_events: ["push".to_owned()].into(),
        allowed_refs: ["refs/heads/main".to_owned()].into(),
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 4,
        memory_mb: 8_192,
        storage_mb: 40_960,
        boot_timeout_seconds: 30,
        idle_timeout_seconds: 30,
        job_timeout_seconds: 60,
        cleanup_timeout_seconds: 10,
        warm_template: None,
        regeneration_workflow: None,
        hot: None,
        reap: true,
    }
}

/// The fixture profile with hot reuse enabled on the recommended lane.
#[must_use]
pub fn hot_profile() -> Profile {
    Profile {
        hot: Some(HotConfig {
            enabled: true,
            lanes: HotLanePolicy::Protected,
            ..HotConfig::default()
        }),
        ..profile()
    }
}

/// A configuration whose single profile enables hot, with one pool slot.
#[must_use]
pub fn hot_config(state_dir: PathBuf) -> Arc<Config> {
    let mut config = (*config(state_dir)).clone();
    config.runtime.max_hot_vms = 1;
    config.profiles = BTreeMap::from([(profile_name(), hot_profile())]);
    Arc::new(config)
}

/// The fixture profile with warm production declared.
#[must_use]
pub fn warm_profile() -> Profile {
    Profile {
        warm_template: Some(
            VmName::new("project-warm").unwrap_or_else(|error| unreachable!("fixture: {error}")),
        ),
        regeneration_workflow: Some("apple.yml".to_owned()),
        ..profile()
    }
}

/// A configuration whose single profile declares warm production.
#[must_use]
pub fn warm_config(state_dir: PathBuf) -> Arc<Config> {
    let mut config = (*config(state_dir)).clone();
    if let Some(tart) = config.runtime.tart_mut() {
        tart.home = Some("/opt/flanforge/tart".into());
    }
    config.profiles = BTreeMap::from([(profile_name(), warm_profile())]);
    Arc::new(config)
}

/// Sizing that matches the fixture profile's ceiling.
#[must_use]
pub fn size() -> GuestSize {
    GuestSize {
        cpu_count: 4,
        memory_mb: 8_192,
        storage_mb: 40_960,
    }
}

#[must_use]
pub fn config(state_dir: PathBuf) -> Arc<Config> {
    let profile = profile();
    let name = profile_name();
    Arc::new(Config {
        logging: flanforge_core::LoggingConfig::default(),
        server: ServerConfig::default(),
        db: flanforge_core::DbConfig::default(),
        webui: flanforge_core::WebuiConfig::default(),
        oidc: OidcConfig {
            issuer: Url::parse("https://git.example/api/actions")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            audience: "flanforged".into(),
            jwks_url: Url::parse("https://git.example/.well-known/jwks")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            jwks_cache_seconds: 300,
            clock_skew_seconds: 30,
        },
        forgejo: flanforge_core::ForgejoConfig {
            api_url: Url::parse("https://git.example/api/v1/")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            api_token_file: state_dir.join("api-token"),
            http_timeout_seconds: 5,
        },
        runtime: flanforge_core::RuntimeConfig {
            state_dir,
            backend: flanforge_core::RuntimeBackendConfig::Tart(flanforge_core::TartConfig {
                path: "/usr/local/bin/tart".into(),
                home: None,
                runner_host_path: Some("/opt/flanforge/forgejo-runner".into()),
            }),
            ssh_path: "/usr/bin/ssh".into(),
            scp_path: "/usr/bin/scp".into(),
            vm_prefix: VmPrefix::new("ci-")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            max_running_vms: 2,
            max_hot_vms: 0,
            poll_seconds: 1,
            reap_interval_hours: 168,
            host_cpu_count: None,
            host_memory_mb: None,
            host_storage_mb: None,
        },
        guest: flanforge_core::GuestConfig {
            channel: flanforge_core::GuestChannelKind::Ssh,
            runner_user: "runner".into(),
            privileged_user: "prunner".into(),
            forgejo_runner_path: "/Users/runner/bin/forgejo-runner".into(),
            ssh: Some(flanforge_core::GuestSshConfig {
                identity_file: "/private/id".into(),
                privileged_identity_file: Some("/private/id".into()),
                known_hosts_file: Some("/private/known_hosts".into()),
                host_key_alias: Some("flanforge-guest".into()),
                connect_timeout_seconds: 5,
                verify_host_key: true,
            }),
        },
        tailscale: flanforge_core::TailscaleConfig::default(),
        profiles: BTreeMap::from([(name, profile)]),
    })
}

#[must_use]
pub fn request() -> AllocationRequest {
    AllocationRequest {
        profile: profile_name(),
        repository: RepositoryName::new("owner/project")
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        run_id: 42,
        run_attempt: 1,
    }
}

#[must_use]
pub fn claims() -> ForgejoClaims {
    ForgejoClaims {
        actor: "developer".into(),
        aud: "flanforged".into(),
        event_name: "push".into(),
        exp: 4_000_000_000,
        iat: 3_999_999_000,
        iss: "https://git.example/api/actions".into(),
        nbf: 3_999_999_000,
        git_ref: "refs/heads/main".into(),
        ref_protected: "true".into(),
        ref_type: "branch".into(),
        repository: "owner/project".into(),
        repository_owner: "owner".into(),
        run_attempt: "1".into(),
        run_id: "42".into(),
        run_number: "7".into(),
        sha: "0123456789012345678901234567890123456789".into(),
        sub: "repo:owner/project:ref:refs/heads/main".into(),
        workflow: "apple.yml".into(),
        workflow_ref: "owner/project/.forgejo/workflows/apple.yml@refs/heads/main".into(),
    }
}
