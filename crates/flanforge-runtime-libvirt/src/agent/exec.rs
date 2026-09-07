use std::time::Duration;

use flanforge_libvirt_wire::AgentExecRequest;
use flanforge_manager::WorkerError;
use flanforge_runtime::GuestProgram;
use uuid::Uuid;

const RUNUSER: &str = "/usr/sbin/runuser";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// The wire refuses anything outside this range, and systemd would never
/// escalate from a zero stop budget.
const MAX_RUNTIME_MAX_SEC: u64 = 86_400;
const STOP_BUDGET_SECONDS: u64 = 10;

const PRIVILEGED_ACCOUNT: &str = "root";

/// The account the agent's root process drops to, as the base image recorded
/// it. The uid comes from the guest rather than from configuration, which is
/// what lets the daemon drop its SSH settings entirely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobAccount {
    name: String,
    uid: u32,
}

impl JobAccount {
    /// # Errors
    /// Returns an error for a privileged or structurally invalid account.
    pub(crate) fn new(name: String, uid: u32) -> Result<Self, WorkerError> {
        // `root` is refused by name as well as by uid: an image that recorded a
        // privileged account is a build the daemon should not run jobs on,
        // whatever number it put beside the name.
        let is_valid = name != PRIVILEGED_ACCOUNT
            && (1..=32).contains(&name.len())
            && name
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            && uid >= 1_000;
        if is_valid {
            Ok(Self { name, uid })
        } else {
            Err(WorkerError::new(
                "guest job account is structurally invalid",
            ))
        }
    }

    /// Fedora's `runuser` deliberately omits `pam_systemd`, so it creates no
    /// logind session and sets no `XDG_RUNTIME_DIR`. Without these two exports
    /// rootless Podman cannot find its socket and every container job fails.
    fn prelude(&self) -> String {
        let uid = self.uid;
        format!(
            "export XDG_RUNTIME_DIR=/run/user/{uid}; \
             export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus; "
        )
    }
}

pub(crate) fn unit_name(handle: Uuid) -> String {
    format!("flanforge-job-{handle}.service")
}

/// One bounded command as a login shell for the job account. `guest-exec` runs
/// as root, so without this wrapper every job would run as root — a privilege
/// escalation introduced by a channel choice.
pub(crate) fn login_shell(
    account: &JobAccount,
    script: &str,
    input: Option<Vec<u8>>,
    is_output_captured: bool,
) -> AgentExecRequest {
    AgentExecRequest {
        path: RUNUSER.to_owned(),
        arguments: vec![
            "-l".to_owned(),
            account.name.clone(),
            "-c".to_owned(),
            format!("{}{script}", account.prelude()),
        ],
        input,
        is_output_captured,
    }
}

/// The job, as a transient unit PID 1 owns. Its name, its lifetime, and its
/// stop are all outside the job account's reach, and `RuntimeMaxSec` ends it
/// even if the daemon never comes back.
pub(crate) fn transient_unit(
    account: &JobAccount,
    script: &str,
    handle: Uuid,
    lifetime: Duration,
    input: Option<Vec<u8>>,
) -> AgentExecRequest {
    let runtime_max = lifetime.as_secs().clamp(1, MAX_RUNTIME_MAX_SEC);
    AgentExecRequest {
        path: SYSTEMD_RUN.to_owned(),
        arguments: vec![
            format!("--unit={}", unit_name(handle)),
            "--pipe".to_owned(),
            "--wait".to_owned(),
            format!("--property=RuntimeMaxSec={runtime_max}"),
            format!("--property=TimeoutStopSec={STOP_BUDGET_SECONDS}"),
            RUNUSER.to_owned(),
            "-l".to_owned(),
            account.name.clone(),
            "-c".to_owned(),
            format!("{}{script}", account.prelude()),
        ],
        input,
        is_output_captured: false,
    }
}

/// `systemctl stop` kills the whole cgroup, so it reaches everything the job
/// started rather than one process group it may have escaped.
pub(crate) fn stop_unit(handle: Uuid) -> AgentExecRequest {
    AgentExecRequest {
        path: SYSTEMCTL.to_owned(),
        arguments: vec!["stop".to_owned(), unit_name(handle)],
        input: None,
        is_output_captured: false,
    }
}

/// A fixed baked program run as root, with its argv preserved. Only the
/// readiness helper takes this form, and it detects its own uid.
pub(crate) fn program(
    path: &str,
    arguments: &[String],
    input: Option<Vec<u8>>,
    is_output_captured: bool,
) -> AgentExecRequest {
    AgentExecRequest {
        path: path.to_owned(),
        arguments: arguments.to_vec(),
        // Carried rather than dropped: the helper refuses stdin on this shape,
        // and a loud refusal beats a guest whose `read` never returns.
        input,
        is_output_captured,
    }
}

/// The one place a channel-neutral command becomes an agent request, so the
/// `runuser` wrapper cannot be forgotten on one path and applied on another.
pub(crate) fn request(
    account: &JobAccount,
    program_form: GuestProgram<'_>,
    input: Option<Vec<u8>>,
    is_output_captured: bool,
) -> AgentExecRequest {
    match program_form {
        GuestProgram::Script(script) => login_shell(account, script, input, is_output_captured),
        GuestProgram::Program { path, arguments } => {
            program(path, arguments, input, is_output_captured)
        }
    }
}
