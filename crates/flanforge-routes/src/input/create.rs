use flanforge_core::{AllocationRequest, ProfileName, RepositoryName, RequestOptions};
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
            cpu_count: self.cpu_count,
            memory_mb: self.memory_mb,
        };
        Ok((request, options))
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
