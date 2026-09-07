use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use tracing::Level;

/// Runs `body` under an INFO-level subscriber and returns what it logged.
///
/// The level is part of the fixture: the daemon runs at info, so a rejection
/// logged below that is not observable at all, which is the regression this
/// harness exists to catch.
///
/// The subscriber is thread-local, so `body` must not move across threads.
pub fn capture_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_writer(Capture(Arc::clone(&buffer)))
        .finish();
    let value = tracing::subscriber::with_default(subscriber, body);
    let logged = buffer
        .lock()
        .map(|logged| String::from_utf8_lossy(&logged).into_owned())
        .unwrap_or_default();
    (value, logged)
}

#[derive(Clone, Debug)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut logged) = self.0.lock() {
            logged.extend_from_slice(buffer);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for Capture {
    type Writer = Self;

    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}
