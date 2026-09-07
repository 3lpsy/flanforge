use flanforge_core::{LibvirtConfig, NetworkMode, RuntimeBackendConfig, VmName};
use tokio_util::sync::CancellationToken;

use crate::{CheckStatus, DoctorCheck, DoctorReport, OperatorError, doctor, import};

fn config(state_dir: &std::path::Path) -> flanforge_core::Config {
    let mut config = (*flanforge_test_support::config(state_dir.to_owned())).clone();
    config.runtime.backend = RuntimeBackendConfig::Libvirt(LibvirtConfig {
        image_manifest_dir: state_dir.join("libvirt/published-bases"),
        ..LibvirtConfig::default()
    });
    config.runtime.max_running_vms = 1;
    config.runtime.host_cpu_count = Some(8);
    config.runtime.host_memory_mb = Some(32_768);
    config.runtime.host_storage_mb = Some(262_144);
    let ssh = config
        .guest
        .ssh
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"));
    ssh.known_hosts_file = None;
    ssh.host_key_alias = None;
    config
        .profiles
        .values_mut()
        .for_each(|profile| profile.network = NetworkMode::Default);
    config
}

#[tokio::test]
async fn cancelled_import_does_not_create_state_or_read_artifacts() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let state_dir = directory.path().join("absent-state");
    let config = config(&state_dir);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let logical =
        VmName::new("flanforge-base").unwrap_or_else(|error| unreachable!("name: {error}"));
    let result = import(
        &config,
        &logical,
        &directory.path().join("absent.qcow2"),
        Some(&directory.path().join("absent.json")),
        &cancellation,
    )
    .await;
    assert!(matches!(result, Err(OperatorError::Cancelled)));
    assert!(!state_dir.exists());
}

#[tokio::test]
async fn doctor_is_read_only_even_when_prerequisites_are_absent() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let state_dir = directory.path().join("absent-state");
    let config = config(&state_dir);
    let report = doctor(&config)
        .await
        .unwrap_or_else(|error| unreachable!("doctor: {error}"));
    assert!(!report.is_healthy());
    assert!(!state_dir.exists());
}

/// A skipped check reports that nothing was checked. It must not fail a
/// healthy host, and it must not be read as a pass either.
#[test]
fn a_skipped_check_is_neither_a_pass_nor_a_failure() {
    let skipped = DoctorCheck::skipped("guest SSH identity", "no [guest.ssh] is configured");
    assert_eq!(skipped.status(), CheckStatus::Skipped);
    assert!(
        DoctorReport::new(vec![
            DoctorCheck::passed("state directory", "ok"),
            skipped.clone(),
        ])
        .is_healthy()
    );
    assert!(
        !DoctorReport::new(vec![
            skipped,
            DoctorCheck::failed("virsh executable", "absent"),
        ])
        .is_healthy()
    );
}

/// The agent channel needs neither an SSH identity nor an ssh binary, and
/// saying "pass" for either would claim something was verified.
#[tokio::test]
async fn the_agent_channel_skips_the_checks_that_do_not_apply_to_it() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let state_dir = directory.path().join("absent-state");
    let mut config = config(&state_dir);
    config.guest.channel = flanforge_core::GuestChannelKind::Agent;
    config.guest.ssh = None;
    let report = doctor(&config)
        .await
        .unwrap_or_else(|error| unreachable!("doctor: {error}"));
    for name in ["guest SSH identity", "SSH executable"] {
        let check = report
            .checks()
            .iter()
            .find(|check| check.name() == name)
            .unwrap_or_else(|| unreachable!("{name} is not reported"));
        assert_eq!(check.status(), CheckStatus::Skipped, "{name}");
    }
}
