use flanforge_wire::{ReapRequest, SweepView};

use crate::{WebuiFault, WebuiServices, views};

/// Plans a sweep, and deletes only when asked.
///
/// # Errors
///
/// Returns `Internal` on a sweep failure.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    request: ReapRequest,
) -> Result<SweepView, WebuiFault> {
    tracing::info!(delete = request.delete, actor, "webui sweep requested");
    let report = services.manager.sweep(!request.delete).await?;
    Ok(views::sweep_view(&report))
}
