use flanforge_core::VmName;
use flanforge_manager::{HostMachine, MachineOwnership, MachineState};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    RuntimeError,
    manifest::{ServiceInstance, intent_for},
};

use super::{
    checks::{GUEST_CONTRACT_SCRIPT, ensure_idle, ensure_not_cancelled},
    cleanup::finish,
    state,
};
use crate::operator::SmokeOutcome;

#[test]
fn admission_refuses_running_and_indeterminate_machines() {
    let machine = |state| HostMachine {
        name: "foreign".to_owned(),
        state,
        age_seconds: None,
        size: None,
        ownership: MachineOwnership::Foreign,
    };
    assert!(ensure_idle(&[machine(MachineState::Stopped)]).is_ok());
    assert!(ensure_idle(&[machine(MachineState::Running)]).is_err());
    assert!(ensure_idle(&[machine(MachineState::Other)]).is_err());
}

#[test]
fn smoke_script_has_no_forgejo_or_container_pull_work() {
    assert!(!GUEST_CONTRACT_SCRIPT.contains("forgejo"));
    assert!(!GUEST_CONTRACT_SCRIPT.contains("podman run"));
    assert!(!GUEST_CONTRACT_SCRIPT.contains("docker run"));
    assert!(GUEST_CONTRACT_SCRIPT.contains("/dev/kvm"));
    assert!(GUEST_CONTRACT_SCRIPT.contains("docker version"));
}

#[test]
fn cancellation_and_cleanup_errors_are_never_hidden() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        ensure_not_cancelled(&cancellation),
        Err(RuntimeError::Cancelled)
    );
    let outcome = SmokeOutcome::new(
        VmName::new("ci-smoke-test").unwrap_or_else(|error| unreachable!("name: {error}")),
        Some(std::net::Ipv4Addr::LOCALHOST.into()),
    );
    let cleanup = RuntimeError::manifest("injected cleanup failure");
    assert_eq!(
        finish(Ok(outcome), Err(cleanup.clone())),
        Err(cleanup.clone())
    );
    assert!(matches!(
        finish(Err(RuntimeError::Cancelled), Err(cleanup)),
        Err(RuntimeError::Cleanup { .. })
    ));
}

#[tokio::test]
async fn smoke_state_round_trips_and_removes_only_its_exact_directory() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let instance = ServiceInstance::load_or_create(directory.path())
        .unwrap_or_else(|error| unreachable!("instance: {error}"));
    let allocation_id = Uuid::new_v4();
    let name = VmName::new(format!("ci-smoke-{allocation_id}"))
        .unwrap_or_else(|error| unreachable!("name: {error}"));
    let manifest = intent_for(
        allocation_id,
        &name,
        &instance,
        directory.path(),
        format!("flanforge-{allocation_id}"),
    )
    .unwrap_or_else(|error| unreachable!("intent: {error}"));
    state::prepare(directory.path(), &manifest, "anchor ssh-ed25519 AAAA\n")
        .await
        .unwrap_or_else(|error| unreachable!("prepare: {error}"));
    assert_eq!(
        state::load_pending(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        Some(manifest.clone())
    );
    state::remove(directory.path(), &manifest)
        .await
        .unwrap_or_else(|error| unreachable!("remove: {error}"));
    assert_eq!(
        state::load_pending(directory.path())
            .await
            .unwrap_or_else(|error| unreachable!("load: {error}")),
        None
    );
}
