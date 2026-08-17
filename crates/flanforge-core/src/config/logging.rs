use super::{ConfigError, LoggingConfig, validate::ensure_path};

pub(super) fn ensure_logging_valid(logging: &LoggingConfig) -> Result<(), ConfigError> {
    if !matches!(
        logging.level.trim().to_ascii_lowercase().as_str(),
        "trace" | "debug" | "info" | "warn" | "error" | "off"
    ) {
        return Err(ConfigError::UnsafeValue {
            field: "logging.level",
        });
    }
    if let Some(path) = &logging.path {
        ensure_path("logging.path", path)?;
    }
    Ok(())
}
