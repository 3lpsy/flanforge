use std::str::FromStr;

use flanforge_core::{AllocationId, unix_time};
use flanforge_wire::AllocationView;

use crate::{WebuiFault, WebuiServices, views};

/// Cancels one allocation through the lifecycle path the service uses.
///
/// # Errors
///
/// Returns `Invalid` for a malformed id or `NotFound` for an unknown one.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    id: &str,
) -> Result<AllocationView, WebuiFault> {
    let id = AllocationId::from_str(id).map_err(|_| WebuiFault::Invalid("invalid id"))?;
    tracing::info!(allocation_id = %id, actor, "webui allocation cancellation requested");
    let cancelled = services.manager.cancel_by_id(id).await?;
    Ok(AllocationView {
        id: cancelled.id.to_string(),
        profile: cancelled.request.profile.as_str().to_owned(),
        repository: views::token(&cancelled.request.repository),
        run_id: cancelled.request.run_id,
        run_attempt: cancelled.request.run_attempt,
        state: views::token(&cancelled.state),
        mode: views::token(&cancelled.mode),
        vm_name: cancelled.vm_name.as_str().to_owned(),
        age_seconds: unix_time().saturating_sub(cancelled.created_at_unix),
        cpu_count: cancelled.size.map(|size| size.cpu_count),
        memory_mb: cancelled.size.map(|size| size.memory_mb),
        storage_mb: cancelled.size.map(|size| size.storage_mb),
        source: cancelled
            .source
            .as_ref()
            .map(|source| source.name.as_str().to_owned()),
    })
}
