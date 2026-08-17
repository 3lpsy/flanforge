use std::io;

/// What a bounded consent probe observed.
///
/// A timeout is an unanswered prompt, which is a different state from a
/// refusal: the daemon hangs on the former and fails fast on the latter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ProbeOutcome {
    Allowed,
    Denied,
    TimedOut,
    Skipped(String),
    Failed(String),
}

impl ProbeOutcome {
    /// `None` is the bound elapsing, which is the unanswered-prompt case.
    pub(super) fn classify(result: Option<io::Result<()>>) -> Self {
        match result {
            None => Self::TimedOut,
            Some(Ok(())) => Self::Allowed,
            Some(Err(error)) => match error.kind() {
                io::ErrorKind::PermissionDenied
                | io::ErrorKind::HostUnreachable
                | io::ErrorKind::NetworkUnreachable => Self::Denied,
                _ => Self::Failed(single_line(&error.to_string())),
            },
        }
    }

    pub(super) fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    pub(super) fn token(&self) -> String {
        match self {
            Self::Allowed => "allowed".to_owned(),
            Self::Denied => "denied".to_owned(),
            Self::TimedOut => "timed-out".to_owned(),
            Self::Skipped(detail) => format!("skipped:{}", single_line(detail)),
            Self::Failed(detail) => format!("failed:{}", single_line(detail)),
        }
    }

    pub(super) fn parse(token: &str) -> Option<Self> {
        match token.trim() {
            "allowed" => Some(Self::Allowed),
            "denied" => Some(Self::Denied),
            "timed-out" => Some(Self::TimedOut),
            other => match other.split_once(':') {
                Some(("skipped", detail)) => Some(Self::Skipped(detail.trim().to_owned())),
                Some(("failed", detail)) => Some(Self::Failed(detail.trim().to_owned())),
                _ => None,
            },
        }
    }
}

/// Reads `<gate>=<outcome>` lines, ignoring anything else on the stream.
pub(super) fn parse_probe_report(output: &str, gate: &str) -> Option<ProbeOutcome> {
    output
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .find(|(name, _)| *name == gate)
        .and_then(|(_, token)| ProbeOutcome::parse(token))
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_owned()
}
