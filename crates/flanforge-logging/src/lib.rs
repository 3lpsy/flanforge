//! Two-phase tracing setup for the daemon.
//!
//! [`init`] installs stdout logging before configuration is available.
//! [`configure`] then applies the configured level and optionally attaches an
//! append-only plain-text file sink to the same subscriber.

use std::{
    fmt::Write as _,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

mod hub;
mod tee;

pub use hub::{LogHub, LogLine};

use tracing_subscriber::{
    EnvFilter, Registry,
    field::RecordFields,
    fmt::{
        FormatFields,
        format::{DefaultFields, Writer},
    },
    prelude::*,
    reload,
};

static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static LOG_HUB: OnceLock<LogHub> = OnceLock::new();

/// The process-wide log tail the web UI streams from. Always present; empty
/// until `init` installs the tee.
pub fn log_hub() -> &'static LogHub {
    LOG_HUB.get_or_init(LogHub::default)
}
static LOG_FILE: Mutex<Option<LogSink>> = Mutex::new(None);

const QUIET_TARGETS: &[&str] = &["h2", "hyper", "reqwest", "rustls"];
const LEVELS: [&str; 6] = ["trace", "debug", "info", "warn", "error", "off"];

/// Installs stdout tracing and the panic hook immediately.
pub fn init() {
    if FILTER_HANDLE.get().is_some() {
        return;
    }
    install_panic_hook();
    let directives = filter_directives("info", std::env::var("RUST_LOG").ok().as_deref());
    let (filter, handle) = reload::Layer::new(EnvFilter::new(&directives));
    let stdout_layer = tracing_subscriber::fmt::layer().with_writer(io::stdout);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .log_internal_errors(true)
        .fmt_fields(PlainFields(DefaultFields::new()))
        .with_writer(|| FileWriter);

    let _ = FILTER_HANDLE.set(handle);
    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .with(tee::HubLayer)
        .init();
}

/// Applies the configured filter and optionally starts teeing to `log_path`.
///
/// # Errors
///
/// Returns an error if the filter cannot be reloaded or the file cannot be
/// opened.
pub fn configure(level: &str, log_path: Option<&Path>) -> Result<(), String> {
    if let Some(handle) = FILTER_HANDLE.get() {
        let directives = filter_directives(level, std::env::var("RUST_LOG").ok().as_deref());
        handle
            .reload(EnvFilter::new(&directives))
            .map_err(|error| format!("cannot reload log filter: {error}"))?;
    }
    ensure_log_file(log_path)?;
    tracing::info!(level = %resolve_level(level), file = log_path.is_some(), "logging configured");
    Ok(())
}

/// Authoritative in both directions: a reload that drops `logging.path` must
/// stop the file trail, not leave the previous sink appending unattended.
pub(crate) fn ensure_log_file(log_path: Option<&Path>) -> Result<(), String> {
    let sink = match log_path {
        Some(path) => Some(
            LogSink::open(path)
                .map_err(|error| format!("cannot open log file {}: {error}", path.display()))?,
        ),
        None => None,
    };
    *LOG_FILE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = sink;
    Ok(())
}

struct LogSink {
    file: File,
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

impl LogSink {
    fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            identity: file_identity(path),
        })
    }

    fn is_current(&self) -> bool {
        #[cfg(unix)]
        {
            self.identity.is_some() && file_identity(&self.path) == self.identity
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    /// Reopens the sink when the configured path no longer names the open file,
    /// so an external rotation cannot silently orphan the audit trail.
    fn ensure_current(&mut self) -> io::Result<()> {
        if self.is_current() {
            return Ok(());
        }
        *self = Self::open(&self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
const fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

struct PlainFields(DefaultFields);

impl<'writer> FormatFields<'writer> for PlainFields {
    fn format_fields<R: RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> std::fmt::Result {
        self.0.format_fields(writer, fields)
    }
}

struct FileWriter;

impl Write for FileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut guard = LOG_FILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sink) = guard.as_mut() {
            sink.ensure_current()?;
            sink.file.write_all(buffer)?;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = LOG_FILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sink) = guard.as_mut() {
            sink.file.flush()?;
        }
        Ok(())
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info
            .location()
            .map_or_else(|| "<unknown>".to_owned(), ToString::to_string);
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%location, "panic: {message}\n{backtrace}");
    }));
}

fn resolve_level(level: &str) -> String {
    let trimmed = level.trim();
    if LEVELS
        .iter()
        .any(|known| trimmed.eq_ignore_ascii_case(known))
    {
        return trimmed.to_ascii_lowercase();
    }
    eprintln!(
        "warning: unknown log level `{level}`; falling back to `info` (accepted: {})",
        LEVELS.join(", ")
    );
    "info".to_owned()
}

fn filter_directives(level: &str, rust_log: Option<&str>) -> String {
    match rust_log {
        Some(value) if !value.is_empty() => value.to_owned(),
        _ => {
            let mut directives = resolve_level(level);
            for target in QUIET_TARGETS {
                let _ = write!(directives, ",{target}=warn");
            }
            directives
        }
    }
}

#[cfg(test)]
mod tests;
