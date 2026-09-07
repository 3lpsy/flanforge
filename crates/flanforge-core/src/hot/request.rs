use serde::{Deserialize, Serialize};

use crate::Profile;

/// What a workflow asked the pool to do. Four meanings on one request field,
/// because they are four answers to one question — "what happens to this
/// guest when the job ends?" — and splitting them across flags would let a
/// workflow spell combinations that mean nothing.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum HotRequest {
    /// The pool is untouched: no claim, no retention, and nothing torn down.
    /// A machine another workflow is relying on must survive a request that
    /// simply did not mention hot.
    #[default]
    Untouched,
    /// Reuse a retained machine if one is idle, and retain this guest after
    /// the job. `age_seconds` is this run's own lifetime for the machine;
    /// `None` takes the profile's configured maximum.
    Retain { age_seconds: Option<u64> },
    /// Tear this profile's retained machines down, then run as an ordinary
    /// allocation that retains nothing.
    Evict,
}

impl HotRequest {
    /// Whether this request wants a machine kept, which is the one question
    /// admission asks before it looks at the pool at all.
    #[must_use]
    pub const fn is_retaining(self) -> bool {
        matches!(self, Self::Retain { .. })
    }

    #[must_use]
    pub const fn is_evicting(self) -> bool {
        matches!(self, Self::Evict)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("requested hot lifetime exceeds its profile ceiling")]
pub struct HotCeilingError;

/// Resolves how long a retained machine may live, beneath the profile's
/// ceiling.
///
/// `hot.max_lifetime_seconds` is the ceiling on what a run may ask for, not
/// the lifetime itself. A request above it is **rejected, not clamped**, for
/// the same reason `resolve_size` rejects an oversized `cpu_count`: silently
/// granting four hours to a workflow that asked for five is worse than saying
/// no.
///
/// A profile that does not enable hot has no ceiling to exceed, so the age is
/// irrelevant: the request is refused silently as `NotEnabled` and the
/// allocation runs as an ordinary one. A refusal never fails an allocation.
///
/// # Errors
///
/// Returns an error when a hot-enabled profile's ceiling is exceeded.
pub fn resolve_hot_age(
    request: HotRequest,
    profile: &Profile,
) -> Result<Option<u64>, HotCeilingError> {
    let HotRequest::Retain { age_seconds } = request else {
        return Ok(None);
    };
    let Some(age_seconds) = age_seconds else {
        return Ok(None);
    };
    let Some(hot) = profile.hot.filter(|hot| hot.enabled) else {
        return Ok(None);
    };
    if age_seconds > hot.max_lifetime_seconds {
        return Err(HotCeilingError);
    }
    Ok(Some(age_seconds))
}
