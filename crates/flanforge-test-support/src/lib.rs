use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use flanforge_core::{
    AllocationRequest, Config, ForgejoClaims, GuestSize, NetworkMode, OidcConfig, Profile,
    ProfileName, RepositoryName, RunnerLabel, ServerConfig, VmName, VmPrefix,
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
        allowed_ref_prefixes: BTreeSet::default(),
        require_protected_ref: true,
        network: NetworkMode::Softnet,
        cpu_count: 4,
        memory_mb: 8_192,
        boot_timeout_seconds: 30,
        idle_timeout_seconds: 30,
        job_timeout_seconds: 60,
        cleanup_timeout_seconds: 10,
        warm_template: None,
        regeneration_workflow: None,
        reap: true,
    }
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
    config.runtime.tart_home = Some("/opt/flanforge/tart".into());
    config.profiles = BTreeMap::from([(profile_name(), warm_profile())]);
    Arc::new(config)
}

/// Sizing that matches the fixture profile's ceiling.
#[must_use]
pub fn size() -> GuestSize {
    GuestSize {
        cpu_count: 4,
        memory_mb: 8_192,
    }
}

#[must_use]
pub fn config(state_dir: PathBuf) -> Arc<Config> {
    let profile = profile();
    let name = profile_name();
    Arc::new(Config {
        logging: flanforge_core::LoggingConfig::default(),
        server: ServerConfig::default(),
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
            tart_path: "/usr/local/bin/tart".into(),
            ssh_path: "/usr/bin/ssh".into(),
            scp_path: "/usr/bin/scp".into(),
            forgejo_runner_host_path: "/opt/flanforge/forgejo-runner".into(),
            vm_prefix: VmPrefix::new("ci-")
                .unwrap_or_else(|error| unreachable!("fixture: {error}")),
            tart_home: None,
            max_running_vms: 2,
            poll_seconds: 1,
            reap_interval_hours: 168,
            host_cpu_count: None,
            host_memory_mb: None,
        },
        guest: flanforge_core::GuestConfig {
            ssh_user: "runner".into(),
            ssh_identity_file: "/private/id".into(),
            ssh_known_hosts_file: Some("/private/known_hosts".into()),
            ssh_host_key_alias: Some("flanforge-guest".into()),
            forgejo_runner_path: "/Users/runner/bin/forgejo-runner".into(),
            ssh_connect_timeout_seconds: 5,
            verify_host_key: true,
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
