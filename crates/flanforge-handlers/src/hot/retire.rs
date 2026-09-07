use flanforge_core::VmName;
use flanforge_wire::{HotGuestView, HotRetireRequest};

use crate::{WebuiFault, WebuiServices};

/// Stops one machine taking new claims, or destroys it outright.
///
/// # Errors
///
/// Returns `Invalid` for a malformed name or `NotFound` for an unclaimed one.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    name: &str,
    request: HotRetireRequest,
) -> Result<Vec<HotGuestView>, WebuiFault> {
    let name = VmName::new(name).map_err(|_| WebuiFault::Invalid("invalid VM name"))?;
    tracing::info!(vm_name = %name, evict = request.evict, actor, "webui hot retirement requested");
    if request.evict {
        services.manager.hot_evict(&name).await?;
    } else {
        services.manager.hot_drain(&name).await?;
    }
    Ok(super::list(services).await)
}
