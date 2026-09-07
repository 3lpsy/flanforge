use crate::{HotRequest, Profile, allocation::AllocationMode, allocation::GuestSize};

/// The request fields that select beneath profile policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RequestOptions {
    pub warm: bool,
    /// What this run asks the pool to do. A preference the profile may refuse,
    /// exactly as `warm` is, and deliberately not folded into
    /// `AllocationMode`: it is orthogonal to which image the guest boots from.
    pub hot: HotRequest,
    pub cpu_count: Option<u8>,
    pub memory_mb: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("request exceeds its profile ceiling")]
pub struct SizeCeilingError;

/// Resolves request sizing beneath the profile ceiling; the profile supplies
/// every value the request omits.
///
/// # Errors
///
/// Returns an error when either requested value exceeds the profile, which is
/// rejected rather than clamped.
pub fn resolve_size(
    options: RequestOptions,
    profile: &Profile,
) -> Result<GuestSize, SizeCeilingError> {
    let cpu_count = options.cpu_count.unwrap_or(profile.cpu_count);
    let memory_mb = options.memory_mb.unwrap_or(profile.memory_mb);
    if cpu_count > profile.cpu_count || memory_mb > profile.memory_mb {
        return Err(SizeCeilingError);
    }
    Ok(GuestSize {
        cpu_count,
        memory_mb,
        storage_mb: profile.storage_mb,
    })
}

/// Decides the mode from the verified workflow claim, never from the body, so
/// "regeneration never boots warm" is unrepresentable.
#[must_use]
pub fn resolve_mode(
    options: RequestOptions,
    profile: &Profile,
    workflow_file: &str,
) -> AllocationMode {
    let is_producer = profile.warm_template.is_some()
        && profile
            .regeneration_workflow
            .as_deref()
            .is_some_and(|declared| declared == workflow_file);
    if is_producer {
        AllocationMode::Regenerate
    } else if options.warm || options.hot.is_retaining() {
        // A retaining request sources like a warm one: "give it warm, and keep
        // it". A profile that declares no warm template falls back to the cold
        // template here exactly as a warm request does.
        AllocationMode::Warm
    } else {
        AllocationMode::Cold
    }
}
