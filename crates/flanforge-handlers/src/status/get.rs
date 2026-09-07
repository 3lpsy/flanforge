use flanforge_wire::StatusView;

use crate::{WebuiServices, views};

/// The dashboard document: the operator snapshot plus the advisories the
/// daemon logs, so what the UI shows is what admission measures against.
pub async fn handle(services: &WebuiServices) -> StatusView {
    let status = services.manager.status_snapshot().await;
    let config = services.config.current();
    StatusView {
        runtime: status.runtime,
        capacity: status.capacity,
        config_generation: status.config_generation,
        restart_pending: status.restart_pending,
        advisories: config
            .advisories()
            .iter()
            .map(flanforge_core::ConfigAdvisory::message)
            .collect(),
        warm_images: status
            .warm_images
            .iter()
            .map(views::warm_image_view)
            .collect(),
        is_warm_image_store_visible: status.is_warm_image_store_visible,
        last_sweep: status.last_sweep.as_ref().map(views::sweep_view),
    }
}
