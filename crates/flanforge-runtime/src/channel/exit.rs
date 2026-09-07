use std::process::ExitStatus;

/// How one guest process ended, in the terms every channel can report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestExit {
    Code(i32),
    Signal(i32),
    /// The channel can no longer observe the process. Never a verdict on the
    /// job: the process may still be running.
    Lost,
}

impl GuestExit {
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Code(0))
    }

    /// Whether this exit may decide the allocation. A lost observation may not.
    #[must_use]
    pub const fn is_verdict(self) -> bool {
        !matches!(self, Self::Lost)
    }

    /// The exit code, or `None` when the process did not carry one.
    #[must_use]
    pub const fn code(self) -> Option<i32> {
        match self {
            Self::Code(code) => Some(code),
            Self::Signal(_) | Self::Lost => None,
        }
    }
}

impl From<ExitStatus> for GuestExit {
    fn from(status: ExitStatus) -> Self {
        status
            .code()
            .map_or_else(|| Self::Signal(termination_signal(status)), Self::Code)
    }
}

/// A status carrying no code was terminated by a signal, which only Unix names.
#[cfg(unix)]
fn termination_signal(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().unwrap_or_default()
}

#[cfg(not(unix))]
const fn termination_signal(_status: ExitStatus) -> i32 {
    0
}

/// One completed command's exit and however much of its output the channel was
/// asked to keep.
#[derive(Debug)]
pub struct GuestOutput {
    exit: GuestExit,
    stdout: Vec<u8>,
    is_truncated: bool,
    stderr: Vec<u8>,
}

impl GuestOutput {
    #[must_use]
    pub const fn new(exit: GuestExit, stdout: Vec<u8>, is_truncated: bool) -> Self {
        Self {
            exit,
            stdout,
            is_truncated,
            stderr: Vec::new(),
        }
    }

    /// The same output with the process's error stream kept for diagnostics.
    #[must_use]
    pub fn with_stderr(mut self, stderr: Vec<u8>) -> Self {
        self.stderr = stderr;
        self
    }

    /// Diagnostic only: what the guest wrote beside the answer, for failure
    /// messages. Empty on channels that do not keep it.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    #[must_use]
    pub const fn exit(&self) -> GuestExit {
        self.exit
    }

    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Whether the guest wrote more than the capture bound allowed, so what is
    /// held is a prefix rather than the whole stream.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.is_truncated
    }
}
