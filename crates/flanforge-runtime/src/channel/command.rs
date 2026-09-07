use std::{path::Path, time::Duration};

use flanforge_manager::WorkerError;

use super::GuestSecret;

/// The largest script the daemon will carry to a guest over any channel.
const MAX_SCRIPT_BYTES: usize = 64 * 1_024;
const MAX_PROGRAM_PATH_BYTES: usize = 4_096;
const MAX_PROGRAM_ARGUMENTS: usize = 16;
const MAX_PROGRAM_ARGUMENT_BYTES: usize = 128 * 1_024;

/// What to run in the guest. Two forms, because the two channels take two
/// shapes natively and forcing one through the other would either lose the argv
/// or widen a channel's executable allow-list to a general shell.
#[derive(Clone, Copy, Debug)]
pub enum GuestProgram<'a> {
    /// A `/bin/sh` script, run as the job account by every channel.
    Script(&'a str),
    /// A fixed absolute path and bounded arguments, run as whatever identity
    /// the channel holds. Only the baked readiness helper uses this form, and
    /// it detects its own uid.
    Program {
        path: &'a str,
        arguments: &'a [String],
    },
}

/// How much of a command's output the daemon keeps. `Discard` is what every
/// long-running and every secret-carrying command uses; `Bounded` is only for
/// short diagnostics, because a captured stream is buffered whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestCapture {
    Discard,
    Bounded(usize),
}

/// One bounded, daemon-owned command and the optional secret its first `read`
/// takes from stdin. The secret stays out of argv by construction.
#[derive(Debug)]
pub struct GuestCommand<'a> {
    program: GuestProgram<'a>,
    stdin: Option<&'a GuestSecret>,
    capture: GuestCapture,
}

impl<'a> GuestCommand<'a> {
    /// # Errors
    /// Returns an error for a structurally invalid program, or when a
    /// secret-carrying command also asks for its output.
    pub fn new(
        program: GuestProgram<'a>,
        stdin: Option<&'a GuestSecret>,
        capture: GuestCapture,
    ) -> Result<Self, WorkerError> {
        ensure_program_valid(program)?;
        // Captured output from a secret-carrying command is never produced, so
        // it can never reach a log.
        if stdin.is_some() && matches!(capture, GuestCapture::Bounded(_)) {
            return Err(WorkerError::new(
                "a guest command carrying a secret cannot capture output",
            ));
        }
        Ok(Self {
            program,
            stdin,
            capture,
        })
    }

    /// The shape every fixed, daemon-owned maintenance script takes: no secret
    /// and no captured output.
    ///
    /// # Errors
    /// Returns an error for an empty, oversized, or NUL-bearing script.
    pub fn quiet(script: &'a str) -> Result<Self, WorkerError> {
        Self::new(GuestProgram::Script(script), None, GuestCapture::Discard)
    }

    #[must_use]
    pub const fn program(&self) -> GuestProgram<'a> {
        self.program
    }

    #[must_use]
    pub const fn stdin(&self) -> Option<&'a GuestSecret> {
        self.stdin
    }

    #[must_use]
    pub const fn capture(&self) -> GuestCapture {
        self.capture
    }
}

fn ensure_program_valid(program: GuestProgram<'_>) -> Result<(), WorkerError> {
    let is_valid = match program {
        GuestProgram::Script(script) => {
            !script.is_empty() && script.len() <= MAX_SCRIPT_BYTES && !script.contains('\0')
        }
        GuestProgram::Program { path, arguments } => {
            is_normal_absolute(path)
                && arguments.len() <= MAX_PROGRAM_ARGUMENTS
                && arguments
                    .iter()
                    .map(String::len)
                    .try_fold(0_usize, usize::checked_add)
                    .is_some_and(|total| total <= MAX_PROGRAM_ARGUMENT_BYTES)
                && !arguments.iter().any(|argument| argument.contains('\0'))
        }
    };
    if is_valid {
        Ok(())
    } else {
        Err(WorkerError::new(
            "guest daemon script is structurally invalid",
        ))
    }
}

fn is_normal_absolute(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && value.len() <= MAX_PROGRAM_PATH_BYTES
        && !value.contains('\0')
        && path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

/// One long-running guest command, the daemon-owned handle a channel names it
/// by, and the budget after which the guest must end it on its own.
///
/// The handle and the lifetime only mean something to a channel that can name a
/// guest-side process independently of its own connection. The SSH channel
/// cannot — its session lifetime is the kill switch — so it ignores both.
#[derive(Debug)]
pub struct GuestSpawn<'a> {
    command: GuestCommand<'a>,
    handle: uuid::Uuid,
    lifetime: Duration,
}

impl<'a> GuestSpawn<'a> {
    /// # Errors
    /// Returns an error for a nil handle, a zero lifetime, or a command that
    /// captures output nothing will read.
    pub fn new(
        command: GuestCommand<'a>,
        handle: uuid::Uuid,
        lifetime: Duration,
    ) -> Result<Self, WorkerError> {
        if handle.is_nil() || lifetime.is_zero() {
            return Err(WorkerError::new(
                "guest spawn identity or lifetime is structurally invalid",
            ));
        }
        // Nothing reads a supervised process's output, and a captured stream
        // would fill its pipe and block the guest.
        if command.capture() != GuestCapture::Discard {
            return Err(WorkerError::new(
                "a supervised guest command cannot capture output",
            ));
        }
        Ok(Self {
            command,
            handle,
            lifetime,
        })
    }

    #[must_use]
    pub const fn command(&self) -> &GuestCommand<'a> {
        &self.command
    }

    #[must_use]
    pub const fn handle(&self) -> uuid::Uuid {
        self.handle
    }

    #[must_use]
    pub const fn lifetime(&self) -> Duration {
        self.lifetime
    }
}
