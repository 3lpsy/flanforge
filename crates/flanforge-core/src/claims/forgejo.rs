use serde::{Deserialize, Serialize};
use thiserror::Error;
use validator::{Validate, ValidationError};

use flanforge_utils::is_safe_git_ref;

use crate::{AllocationRequest, Profile};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct ForgejoClaims {
    #[validate(length(min = 1, max = 100))]
    #[validate(custom(function = "validate_text"))]
    pub actor: String,
    #[validate(length(min = 1, max = 256))]
    #[validate(custom(function = "validate_text"))]
    pub aud: String,
    #[validate(length(min = 1, max = 64))]
    #[validate(custom(function = "validate_token"))]
    pub event_name: String,
    #[validate(range(min = 1))]
    pub exp: u64,
    #[validate(range(min = 1))]
    pub iat: u64,
    #[validate(length(min = 1, max = 2_048))]
    #[validate(custom(function = "validate_text"))]
    pub iss: String,
    #[validate(range(min = 1))]
    pub nbf: u64,
    #[serde(rename = "ref")]
    #[validate(custom(function = "validate_git_ref"))]
    pub git_ref: String,
    #[validate(custom(function = "validate_boolean"))]
    pub ref_protected: String,
    #[validate(length(min = 1, max = 32))]
    #[validate(custom(function = "validate_token"))]
    pub ref_type: String,
    #[validate(custom(function = "validate_repository"))]
    pub repository: String,
    #[validate(length(min = 1, max = 100))]
    #[validate(custom(function = "validate_name"))]
    pub repository_owner: String,
    #[validate(custom(function = "validate_positive_decimal"))]
    pub run_attempt: String,
    #[validate(custom(function = "validate_positive_decimal"))]
    pub run_id: String,
    #[validate(custom(function = "validate_positive_decimal"))]
    pub run_number: String,
    #[validate(custom(function = "validate_sha"))]
    pub sha: String,
    #[validate(length(min = 1, max = 512))]
    #[validate(custom(function = "validate_text"))]
    pub sub: String,
    #[validate(length(min = 1, max = 128))]
    #[validate(custom(function = "validate_text"))]
    pub workflow: String,
    #[validate(length(min = 1, max = 512))]
    #[validate(custom(function = "validate_text"))]
    pub workflow_ref: String,
}

impl ForgejoClaims {
    /// Checks request fields and policy against the signed Forgejo claims.
    ///
    /// # Errors
    ///
    /// Returns the first authorization dimension that does not match.
    pub fn ensure_authorized(
        &self,
        profile: &Profile,
        request: &AllocationRequest,
    ) -> Result<(), AuthorizationError> {
        self.validate()
            .map_err(|_| AuthorizationError::MalformedClaims)?;
        request
            .validate()
            .map_err(|_| AuthorizationError::MalformedClaims)?;
        if self.repository != request.repository.as_str()
            || self.repository != profile.repository.as_str()
            || self.repository_owner != request.repository.owner()
        {
            return Err(AuthorizationError::Repository);
        }
        if self.run_id.parse::<u64>().ok() != Some(request.run_id)
            || self.run_attempt.parse::<u32>().ok() != Some(request.run_attempt)
            || request.run_id == 0
            || request.run_attempt == 0
        {
            return Err(AuthorizationError::RunIdentity);
        }
        if !self.workflow_identity().is_some_and(|(path, git_ref)| {
            profile.allowed_workflows.contains(path) && git_ref == self.git_ref
        }) {
            return Err(AuthorizationError::Workflow);
        }
        if !profile.allowed_events.contains(&self.event_name) {
            return Err(AuthorizationError::Event);
        }
        if !profile.allowed_refs.contains(&self.git_ref)
            && !profile
                .allowed_ref_prefixes
                .iter()
                .any(|prefix| self.git_ref.starts_with(prefix))
        {
            return Err(AuthorizationError::GitRef);
        }
        let is_protected = self
            .ref_protected
            .parse::<bool>()
            .map_err(|_| AuthorizationError::MalformedClaims)?;
        if profile.require_protected_ref && !is_protected {
            return Err(AuthorizationError::ProtectedRef);
        }
        let expected_subject = if self.event_name == "pull_request" {
            format!("repo:{}:pull_request", self.repository)
        } else {
            format!("repo:{}:ref:{}", self.repository, self.git_ref)
        };
        if self.sub != expected_subject {
            return Err(AuthorizationError::Subject);
        }
        Ok(())
    }

    /// The workflow file inside this repository's workflow directory, or None
    /// when the claim is not shaped like one.
    #[must_use]
    pub fn workflow_file(&self) -> Option<&str> {
        self.workflow_identity().map(|(path, _)| path)
    }

    /// Parses the workflow claim once, for both authorization and production.
    fn workflow_identity(&self) -> Option<(&str, &str)> {
        let prefix = format!("{}/.forgejo/workflows/", self.repository);
        self.workflow_ref.strip_prefix(&prefix)?.split_once('@')
    }
}

fn validate_text(value: &str) -> Result<(), ValidationError> {
    if value.bytes().all(|byte| !byte.is_ascii_control()) {
        Ok(())
    } else {
        Err(ValidationError::new("text"))
    }
}

fn validate_token(value: &str) -> Result<(), ValidationError> {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(())
    } else {
        Err(ValidationError::new("token"))
    }
}

fn validate_name(value: &str) -> Result<(), ValidationError> {
    if value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(ValidationError::new("name"))
    }
}

fn validate_git_ref(value: &str) -> Result<(), ValidationError> {
    if is_safe_git_ref(value) {
        Ok(())
    } else {
        Err(ValidationError::new("git_ref"))
    }
}

fn validate_boolean(value: &str) -> Result<(), ValidationError> {
    if matches!(value, "true" | "false") {
        Ok(())
    } else {
        Err(ValidationError::new("boolean"))
    }
}

fn validate_repository(value: &str) -> Result<(), ValidationError> {
    crate::RepositoryName::new(value)
        .map(|_| ())
        .map_err(|_| ValidationError::new("repository"))
}

fn validate_positive_decimal(value: &str) -> Result<(), ValidationError> {
    if !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|number| number > 0)
    {
        Ok(())
    } else {
        Err(ValidationError::new("positive_decimal"))
    }
}

fn validate_sha(value: &str) -> Result<(), ValidationError> {
    if matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ValidationError::new("sha"))
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorizationError {
    #[error("repository claim is not authorized")]
    Repository,
    #[error("run identity does not match the request")]
    RunIdentity,
    #[error("workflow claim is not authorized")]
    Workflow,
    #[error("event claim is not authorized")]
    Event,
    #[error("git ref claim is not authorized")]
    GitRef,
    #[error("a protected ref is required")]
    ProtectedRef,
    #[error("subject claim is inconsistent")]
    Subject,
    #[error("OIDC claims are malformed")]
    MalformedClaims,
}
