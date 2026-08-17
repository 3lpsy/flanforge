use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError, ValidationErrors};

use crate::IdentifierError;

/// Identity of the base a warm image was produced from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BaseFingerprint(String);

/// Size and nanosecond mtime of one file the fingerprint covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileStat {
    pub len: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
}

impl BaseFingerprint {
    pub const MAX_LEN: usize = 128;

    /// Creates a validated fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is empty, too long, or not hexadecimal.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if !(1..=Self::MAX_LEN).contains(&value.len()) {
            return Err(IdentifierError::Length {
                kind: "base fingerprint",
                min: 1,
                max: Self::MAX_LEN,
            });
        }
        if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(IdentifierError::Character {
                kind: "base fingerprint",
            });
        }
        Ok(Self(value))
    }

    /// Encodes both stats losslessly, so any difference invalidates the image.
    #[must_use]
    pub fn from_stats(disk: FileStat, config: FileStat) -> Self {
        Self(format!(
            "{:016x}{:016x}{:08x}{:016x}{:016x}{:08x}",
            disk.len,
            disk.mtime_secs,
            disk.mtime_nanos,
            config.len,
            config.mtime_secs,
            config.mtime_nanos
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BaseFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Validate for BaseFingerprint {
    fn validate(&self) -> Result<(), ValidationErrors> {
        Self::new(self.0.clone()).map(|_| ()).map_err(|_| {
            let mut errors = ValidationErrors::new();
            errors.add("value", ValidationError::new("base_fingerprint"));
            errors
        })
    }
}

/// Unequal unless both sides are known, so an unreadable base fails cold.
#[must_use]
pub fn is_fingerprint_match(
    recorded: Option<&BaseFingerprint>,
    current: Option<&BaseFingerprint>,
) -> bool {
    matches!((recorded, current), (Some(recorded), Some(current)) if recorded == current)
}
