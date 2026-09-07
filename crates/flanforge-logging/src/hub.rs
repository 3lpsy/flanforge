use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::broadcast;

/// Backlog a fresh subscriber may replay, and the live channel's slack before
/// a slow subscriber starts losing lines.
const BACKLOG_LINES: usize = 1_000;
const CHANNEL_LINES: usize = 256;

/// One captured log record, as the web UI renders it.
#[derive(Clone, Debug)]
pub struct LogLine {
    /// Monotonic per-process sequence; the dedupe key between backlog and
    /// live stream.
    pub seq: u64,
    pub ts_unix_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// A bounded in-memory tail of the daemon's own log plus a broadcast of new
/// lines: the substrate the `/api/v1/logs/stream` endpoint rides on.
#[derive(Debug)]
pub struct LogHub {
    inner: Mutex<HubState>,
    sender: broadcast::Sender<LogLine>,
}

#[derive(Debug)]
struct HubState {
    backlog: VecDeque<LogLine>,
    next_seq: u64,
}

impl Default for LogHub {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HubState {
                backlog: VecDeque::with_capacity(BACKLOG_LINES),
                next_seq: 0,
            }),
            sender: broadcast::channel(CHANNEL_LINES).0,
        }
    }
}

impl LogHub {
    pub(crate) fn push(&self, level: &str, target: &str, message: String) {
        let ts_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let line = LogLine {
            seq: inner.next_seq,
            ts_unix_ms,
            level: level.to_owned(),
            target: target.to_owned(),
            message,
        };
        inner.next_seq += 1;
        if inner.backlog.len() == BACKLOG_LINES {
            inner.backlog.pop_front();
        }
        inner.backlog.push_back(line.clone());
        drop(inner);
        let _ = self.sender.send(line);
    }

    /// Subscribes *before* snapshotting the backlog, so a line landing in
    /// between appears in one of the two and `seq` dedupes the overlap.
    #[must_use]
    pub fn subscribe(&self, backlog_lines: usize) -> (Vec<LogLine>, broadcast::Receiver<LogLine>) {
        let receiver = self.sender.subscribe();
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let skip = inner.backlog.len().saturating_sub(backlog_lines);
        (inner.backlog.iter().skip(skip).cloned().collect(), receiver)
    }
}
