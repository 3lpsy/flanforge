use std::{net::IpAddr, path::PathBuf};

use flanforge_core::GuestConfig;

use super::{SshChannel, options};
use crate::{
    channel::{GuestChannel, GuestCommand},
    guest::GuestSession,
};

/// Every option the daemon owns, before host-key policy and identity.
const HARDENING: [&str; 22] = [
    "-F",
    "/dev/null",
    "-o",
    "GlobalKnownHostsFile=/dev/null",
    "-o",
    "BatchMode=yes",
    "-o",
    "IdentitiesOnly=yes",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ForwardX11=no",
    "-o",
    "ControlMaster=no",
    "-o",
    "ControlPath=none",
    "-o",
    "PermitLocalCommand=no",
    "-o",
    "ProxyCommand=none",
    "-o",
    "LogLevel=ERROR",
];

fn guest_config() -> GuestConfig {
    let directory = std::env::temp_dir();
    (*flanforge_test_support::config(directory)).clone().guest
}

fn channel(config: &GuestConfig) -> SshChannel {
    SshChannel::new(config, "/usr/bin/ssh".into(), "/usr/bin/scp".into())
        .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn ssh_mut(config: &mut GuestConfig) -> &mut flanforge_core::GuestSshConfig {
    config
        .ssh
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"))
}

fn expected(tail: &[&str]) -> Vec<String> {
    HARDENING
        .iter()
        .chain(tail.iter())
        .map(|value| (*value).to_owned())
        .collect()
}

/// The snapshot the extraction is measured against: no later refactor may move
/// a byte of the option vector without this failing.
#[test]
fn a_pinned_anchor_and_alias_produce_the_whole_option_vector() {
    assert_eq!(
        channel(&guest_config()).base_arguments(),
        expected(&[
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UserKnownHostsFile=\"/private/known_hosts\"",
            "-o",
            "HostKeyAlias=flanforge-guest",
            "-o",
            "ConnectTimeout=5",
            "-i",
            "/private/id",
        ])
    );
}

#[test]
fn an_unset_anchor_fails_closed_while_verification_is_enabled() {
    let mut config = guest_config();
    ssh_mut(&mut config).known_hosts_file = None;
    ssh_mut(&mut config).host_key_alias = None;
    assert_eq!(
        channel(&config).base_arguments(),
        expected(&[
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UserKnownHostsFile=\"/dev/null\"",
            "-o",
            "ConnectTimeout=5",
            "-i",
            "/private/id",
        ])
    );
}

#[test]
fn disabled_verification_drops_only_the_host_key_pinning() {
    let mut config = guest_config();
    ssh_mut(&mut config).verify_host_key = false;
    assert_eq!(
        channel(&config).base_arguments(),
        expected(&[
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=5",
            "-i",
            "/private/id",
        ])
    );
}

/// An allocation-pinned session carries the host key its seed injected, so it
/// re-enables verification even when configuration turned it off.
#[test]
fn an_allocation_pinned_session_overrides_the_configured_anchor() {
    let mut config = guest_config();
    ssh_mut(&mut config).verify_host_key = false;
    let session = GuestSession::allocation_pinned(
        IpAddr::from([192, 0, 2, 10]),
        PathBuf::from("/state/allocations/one/known_hosts"),
        "flanforge-one".to_owned(),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let channel = channel(&config);
    let settings = channel.settings_for(&session);
    assert_eq!(
        options::base_arguments(&settings),
        expected(&[
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UserKnownHostsFile=\"/state/allocations/one/known_hosts\"",
            "-o",
            "HostKeyAlias=flanforge-one",
            "-o",
            "ConnectTimeout=5",
            "-i",
            "/private/id",
        ])
    );
}

/// SSH carries commands over the network, so a session with no address names a
/// guest this channel could never have reached. It refuses rather than
/// inventing one.
#[tokio::test]
async fn the_ssh_channel_refuses_a_session_that_has_no_address() {
    let config = guest_config();
    let channel = channel(&config);
    let session = GuestSession::agent_pinned(
        PathBuf::from("/state/allocations/one/known_hosts"),
        "flanforge-one".to_owned(),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let command =
        GuestCommand::quiet("true").unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(channel.run(&session, &command).await.is_err());
    assert!(
        channel
            .ensure_ready(&session, std::time::Duration::from_millis(1))
            .await
            .is_err()
    );
}
