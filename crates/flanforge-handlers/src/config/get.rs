use flanforge_config::EditableDocument;
use flanforge_core::{RuntimeBackendKind, is_ui_editable};
use flanforge_wire::{ConfigFieldView, ConfigView};

use crate::{WebuiFault, WebuiServices};

/// The schema-driven view: the whole document as written (placeholders stay
/// placeholders, so secrets never leave), the running values for showing what
/// an unset key resolves to, and every supported key with its editability.
///
/// # Errors
///
/// Returns `Internal` when the file cannot be read.
pub async fn handle(services: &WebuiServices) -> Result<ConfigView, WebuiFault> {
    // Read without the write lock: viewing must work on a deployment whose
    // configuration is a read-only mount managed outside the daemon.
    let document = EditableDocument::open_read_only(&services.config_path)
        .await
        .map_err(|error| {
            tracing::error!(%error, "cannot open the configuration for the webui");
            WebuiFault::Internal
        })?;
    let written = serde_json::to_value(document.full_view()).map_err(|_| WebuiFault::Internal)?;
    let current = services.config.current();
    let effective = serde_json::to_value(current.as_ref()).map_err(|_| WebuiFault::Internal)?;
    let backend_kind = match current.runtime.backend_kind() {
        RuntimeBackendKind::Tart => "tart",
        RuntimeBackendKind::Libvirt => "libvirt",
    };
    let schema = flanforge_config::view_fields(backend_kind)
        .into_iter()
        .map(|(path, kind)| {
            let segments: Vec<&str> = path.split('.').collect();
            ConfigFieldView {
                editable: is_ui_editable(&segments),
                kind: kind.wire_name().to_owned(),
                path,
            }
        })
        .collect();
    Ok(ConfigView {
        document: written,
        effective,
        schema,
        version: document.version(),
        restart_pending: pending(services),
        generation: services.config.generation(),
        writable: EditableDocument::is_writable(&services.config_path).await,
    })
}

pub(super) fn pending(services: &WebuiServices) -> Vec<String> {
    services
        .config
        .restart_pending()
        .iter()
        .map(|field| (*field).to_owned())
        .collect()
}
