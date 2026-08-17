use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use validator::{Validate, ValidationError, ValidationErrors};

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum IdentifierError {
    #[error("{kind} must be between {min} and {max} ASCII characters")]
    Length {
        kind: &'static str,
        min: usize,
        max: usize,
    },
    #[error("{kind} contains an invalid character")]
    Character { kind: &'static str },
    #[error("repository must have the form owner/name")]
    Repository,
    #[error("VM prefix must end with '-'")]
    VmPrefix,
}

macro_rules! identifier {
    ($name:ident, $kind:literal, $min:expr, $max:expr, $valid:expr) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub const MAX_LEN: usize = $max;

            #[doc = concat!("Creates a validated ", $kind, ".")]
            ///
            /// # Errors
            ///
            /// Returns an error when the value is too long or contains a
            /// character outside the identifier alphabet.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let value = value.into();
                if !($min..=$max).contains(&value.len()) {
                    return Err(IdentifierError::Length {
                        kind: $kind,
                        min: $min,
                        max: $max,
                    });
                }
                if !value
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                    || !value.bytes().all($valid)
                {
                    return Err(IdentifierError::Character { kind: $kind });
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Validate for $name {
            fn validate(&self) -> Result<(), ValidationErrors> {
                Self::new(self.0.clone())
                    .map(|_| ())
                    .map_err(|_| invalid_identifier())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

fn invalid_identifier() -> ValidationErrors {
    let mut errors = ValidationErrors::new();
    errors.add("value", ValidationError::new("identifier"));
    errors
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

identifier!(ProfileName, "profile name", 1, 32, is_name_byte);
identifier!(RunnerLabel, "runner label", 1, 64, is_name_byte);
identifier!(VmName, "VM name", 1, 80, is_name_byte);
identifier!(VmPrefix, "VM prefix", 3, 24, is_name_byte);

impl VmName {
    /// Appends a reserved suffix to a validated name.
    ///
    /// # Errors
    ///
    /// Returns an error when the combined name leaves the alphabet or exceeds
    /// the maximum VM-name length.
    pub fn with_suffix(&self, suffix: &str) -> Result<Self, IdentifierError> {
        Self::new(format!("{}{suffix}", self.0))
    }
}

impl VmPrefix {
    /// Ensures a prefix cannot merge with the first generated name segment.
    ///
    /// # Errors
    ///
    /// Returns an error unless the prefix ends in a hyphen.
    pub fn ensure_valid(&self) -> Result<(), IdentifierError> {
        if self.as_str().ends_with('-') {
            Ok(())
        } else {
            Err(IdentifierError::VmPrefix)
        }
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepositoryName(String);

impl RepositoryName {
    /// Creates a validated `owner/repository` identifier.
    ///
    /// # Errors
    ///
    /// Returns an error unless there are exactly two safe path segments.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.len() > 201 {
            return Err(IdentifierError::Repository);
        }
        let mut parts = value.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository = parts.next().unwrap_or_default();
        if parts.next().is_some()
            || owner.is_empty()
            || repository.is_empty()
            || owner.len() > 100
            || repository.len() > 100
            || matches!(owner, "." | "..")
            || matches!(repository, "." | "..")
            || !owner
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !repository
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !owner.bytes().all(is_name_byte)
            || !repository.bytes().all(is_name_byte)
        {
            return Err(IdentifierError::Repository);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        self.0.split_once('/').map_or("", |(owner, _)| owner)
    }

    #[must_use]
    pub fn repository(&self) -> &str {
        self.0
            .split_once('/')
            .map_or("", |(_, repository)| repository)
    }
}

impl fmt::Debug for RepositoryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RepositoryName")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RepositoryName {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Validate for RepositoryName {
    fn validate(&self) -> Result<(), ValidationErrors> {
        Self::new(self.0.clone())
            .map(|_| ())
            .map_err(|_| invalid_identifier())
    }
}

impl Serialize for RepositoryName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepositoryName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}
