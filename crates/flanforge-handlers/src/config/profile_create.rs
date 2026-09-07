use flanforge_core::{Profile, ProfileName};
use flanforge_wire::{ConfigUpdateResult, ProfileCreate};

use super::finish::{fault_from_edit, finish_edit, open_document};
use crate::{WebuiFault, WebuiServices};

/// Creates a whole `[profiles.<name>]` table through the same
/// validate-then-atomically-write path the CLI's `profile create` uses.
///
/// # Errors
///
/// Returns `Conflict` for a stale version, `Rejected` for a bad name, an
/// existing profile, or a profile the document refuses, or `Internal`.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    name: &str,
    request: &ProfileCreate,
) -> Result<ConfigUpdateResult, WebuiFault> {
    let name = ProfileName::new(name)
        .map_err(|error| WebuiFault::Rejected(format!("invalid profile name: {error}")))?;
    // Typed deserialization is the field validation: every refusal names the
    // field serde stopped on.
    let profile: Profile = serde_json::from_value(request.profile.clone())
        .map_err(|error| WebuiFault::Rejected(format!("invalid profile: {error}")))?;
    let document = open_document(services).await?;
    let mut reloads = services.config.subscribe();
    reloads.mark_unchanged();
    document
        .create_profile(&request.version, name.as_str(), &profile)
        .await
        .map_err(|error| fault_from_edit(error, actor))?;
    tracing::info!(profile = %name, actor, "profile created through the webui");
    finish_edit(services, actor, &[format!("profiles.{name}")], &mut reloads).await
}
