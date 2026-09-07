use std::time::Duration;

use flanforge_libvirt_wire::{HelperFailureCode, OwnershipManifest};
use flanforge_manager::{HostMachine, MachineState};
use flanforge_runtime::{GuestControl, GuestSession};
use tokio_util::sync::CancellationToken;

use crate::{RuntimeError, actor::LibvirtActor};

pub(super) const GUEST_CONTRACT_SCRIPT: &str = r#"set -eu
test "$(id -u)" -ne 0
# Catches a channel that changed uid without giving the job its own login: the
# agent executes as root, and a missing `runuser -l` leaves HOME at /root.
test "$HOME" = "$(getent passwd "$(id -u)" | cut -d: -f6)"
test -d "$HOME"
podman info --format '{{.Host.Security.Rootless}}' | grep -Fxq true
systemctl --user is-active --quiet podman.socket
runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
socket="$runtime_dir/podman/podman.sock"
test -S "$socket"
DOCKER_HOST="unix://$socket" docker version --format '{{.Client.Version}} {{.Server.Version}}' | grep -Eq '^[^ ]+ [^ ]+$'
curl --fail --silent --unix-socket "$socket" http://d/_ping | grep -Fxq OK
test ! -S /run/libvirt/libvirt-sock
test ! -S /run/podman/podman.sock
test ! -S /var/run/docker.sock
test ! -e /dev/kvm
"#;

pub(super) fn ensure_idle(machines: &[HostMachine]) -> Result<(), RuntimeError> {
    if machines
        .iter()
        .any(|machine| matches!(machine.state, MachineState::Running | MachineState::Other))
    {
        return Err(RuntimeError::capacity(
            "standalone smoke requires a host without running or indeterminate VMs",
        ));
    }
    Ok(())
}

pub(super) async fn wait_for_address(
    actor: &LibvirtActor,
    manifest: &OwnershipManifest,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
    poll: Duration,
) -> Result<std::net::IpAddr, RuntimeError> {
    loop {
        ensure_not_cancelled(cancellation)?;
        let request = actor.address(
            manifest.clone(),
            remaining(deadline)?.min(Duration::from_secs(6)),
        );
        let result = tokio::select! {
            () = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
            result = request => result,
        };
        match result {
            Ok(Some(address)) => return Ok(address),
            // Unavailable retries for the same reason the worker's boot wait
            // retries it: a libvirtd restart mid-boot heals on the next poll.
            Ok(None)
            | Err(
                RuntimeError::Transient { .. }
                | RuntimeError::Helper {
                    code: HelperFailureCode::Transient | HelperFailureCode::Unavailable,
                    ..
                },
            ) => {}
            Err(error) => return Err(error),
        }
        tokio::select! {
            () = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
            () = tokio::time::sleep(poll) => {}
        }
    }
}

pub(super) async fn wait_guest_ready(
    control: &GuestControl,
    session: &GuestSession,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<(), RuntimeError> {
    let ready = control.wait_session_ready(session, remaining(deadline)?);
    tokio::select! {
        () = cancellation.cancelled() => Err(RuntimeError::Cancelled),
        result = ready => result.map_err(RuntimeError::guest),
    }
}

pub(super) async fn check_guest(
    control: &GuestControl,
    session: &GuestSession,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<(), RuntimeError> {
    let script = tokio::time::timeout_at(
        deadline,
        control.run_session_script(session, GUEST_CONTRACT_SCRIPT),
    );
    tokio::select! {
        () = cancellation.cancelled() => Err(RuntimeError::Cancelled),
        result = script => result
            .map_err(|_| RuntimeError::Deadline)?
            .map_err(RuntimeError::guest),
    }
}

pub(super) fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), RuntimeError> {
    if cancellation.is_cancelled() {
        Err(RuntimeError::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn remaining(deadline: tokio::time::Instant) -> Result<Duration, RuntimeError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(RuntimeError::Deadline)
}
