use flanforge_core::{AllocationRequest, HotRequest, ProfileName, RepositoryName, RequestOptions};
use serde::Deserialize;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::error::ApiError;

/// Ingress bounds only: the profile ceiling is enforced by the manager, which
/// is the only place the profile is known.
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    #[validate(nested)]
    profile: ProfileName,
    #[validate(nested)]
    repository: RepositoryName,
    #[validate(nested)]
    run_id: RunId,
    #[validate(range(min = 1))]
    run_attempt: u32,
    #[serde(default)]
    warm: bool,
    /// What to do with the pool. A preference: the profile decides whether it
    /// means anything, and a refusal falls back to an ordinary allocation
    /// rather than failing this one.
    #[serde(default)]
    #[validate(nested)]
    hot: Hot,
    #[validate(range(min = 1, max = 64))]
    cpu_count: Option<u8>,
    #[validate(range(min = 2_048, max = 131_072))]
    memory_mb: Option<u32>,
}

impl CreateBody {
    pub(crate) fn into_parts(self) -> Result<(AllocationRequest, RequestOptions), ApiError> {
        let run_id = match self.run_id {
            RunId::Number(value) => value,
            RunId::Text(value) => value.parse().map_err(|_| ApiError::BadRequest)?,
        };
        if run_id == 0 || self.run_attempt == 0 {
            return Err(ApiError::BadRequest);
        }
        let request = AllocationRequest {
            profile: self.profile,
            repository: self.repository,
            run_id,
            run_attempt: self.run_attempt,
        };
        request.validate().map_err(|_| ApiError::BadRequest)?;
        let options = RequestOptions {
            warm: self.warm,
            hot: self.hot.into_request().ok_or(ApiError::BadRequest)?,
            cpu_count: self.cpu_count,
            memory_mb: self.memory_mb,
        };
        Ok((request, options))
    }
}

/// One field, four meanings, decoded the way `RunId` already decodes a value
/// a workflow may spell two ways. The string arm is closed to one word: any
/// other text is a bad request rather than a silently ignored setting.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Hot {
    Flag(bool),
    AgeSeconds(u64),
    Action(String),
}

impl Default for Hot {
    fn default() -> Self {
        Self::Flag(false)
    }
}

impl Hot {
    /// `None` for a value outside the four meanings, which the caller turns
    /// into a bad request. Zero is not a synonym for false: a run that asks
    /// for a zero-second machine has asked for something incoherent.
    fn into_request(self) -> Option<HotRequest> {
        match self {
            Self::Flag(false) => Some(HotRequest::Untouched),
            Self::Flag(true) => Some(HotRequest::Retain { age_seconds: None }),
            Self::AgeSeconds(0) => None,
            Self::AgeSeconds(age_seconds) => Some(HotRequest::Retain {
                age_seconds: Some(age_seconds),
            }),
            Self::Action(action) if action == "evict" => Some(HotRequest::Evict),
            Self::Action(_) => None,
        }
    }
}

impl Validate for Hot {
    fn validate(&self) -> Result<(), ValidationErrors> {
        // Bounded here so an oversized number or a long string is refused at
        // ingress; the profile ceiling is the manager's to enforce.
        let is_valid = match self {
            Self::Flag(_) => true,
            Self::AgeSeconds(value) => (1..=604_800).contains(value),
            Self::Action(value) => value == "evict",
        };
        if is_valid {
            Ok(())
        } else {
            let mut errors = ValidationErrors::new();
            errors.add("hot", ValidationError::new("hot_request"));
            Err(errors)
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RunId {
    Number(u64),
    Text(String),
}

impl Validate for RunId {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let is_valid = match self {
            Self::Number(value) => *value > 0,
            Self::Text(value) => {
                !value.is_empty()
                    && value.len() <= 20
                    && value.bytes().all(|byte| byte.is_ascii_digit())
                    && value.parse::<u64>().is_ok_and(|number| number > 0)
            }
        };
        if is_valid {
            Ok(())
        } else {
            let mut errors = ValidationErrors::new();
            errors.add("run_id", ValidationError::new("positive_integer"));
            Err(errors)
        }
    }
}
