use flanforge_core::ProfileName;
use flanforge_wire::{ProfileDelete, ProfileDeleteResult};

use super::finish::{fault_from_edit, finish_edit, open_document};
use crate::{WebuiFault, WebuiServices};

/// Removes a `[profiles.<name>]` table. Hot machines still serving the
/// profile are reported, not blocked: the reload's drain path retires them,
/// exactly as it would after an editor-made removal.
///
/// # Errors
///
/// Returns `Conflict` for a stale version, `Rejected` for a bad name, a
/// missing profile, or a document that refuses the removal, or `Internal`.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    name: &str,
    request: &ProfileDelete,
) -> Result<ProfileDeleteResult, WebuiFault> {
    let name = ProfileName::new(name)
        .map_err(|error| WebuiFault::Rejected(format!("invalid profile name: {error}")))?;
    // Read before the write so the answer names the machines the removal
    // drains, not the pool after the drain already started.
    let hot_machines: Vec<String> = services
        .manager
        .hot_list()
        .await
        .into_iter()
        .filter(|guest| guest.profile == name)
        .map(|guest| guest.vm_name.to_string())
        .collect();
    let document = open_document(services).await?;
    let mut reloads = services.config.subscribe();
    reloads.mark_unchanged();
    document
        .remove_profile(&request.version, name.as_str())
        .await
        .map_err(|error| fault_from_edit(error, actor))?;
    tracing::info!(profile = %name, actor, "profile removed through the webui");
    let result = finish_edit(services, actor, &[format!("profiles.{name}")], &mut reloads).await?;
    Ok(ProfileDeleteResult {
        version: result.version,
        restart_pending: result.restart_pending,
        reloaded: result.reloaded,
        hot_machines,
    })
}
