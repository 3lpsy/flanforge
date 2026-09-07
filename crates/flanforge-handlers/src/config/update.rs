use flanforge_config::EditValue;
use flanforge_core::is_ui_editable;
use flanforge_wire::{ConfigUpdate, ConfigUpdateResult};

use super::finish::{fault_from_edit, finish_edit, open_document};
use crate::{WebuiFault, WebuiServices};

/// Applies allowlisted configuration changes through the same
/// validate-then-atomically-write path the CLI uses, then nudges the reload
/// watcher and waits briefly for the new generation.
///
/// # Errors
///
/// Returns `Conflict` for a stale version, `Rejected` for a refused key,
/// value, or a document that no longer validates, or `Internal`.
pub async fn handle(
    services: &WebuiServices,
    actor: &str,
    request: &ConfigUpdate,
) -> Result<ConfigUpdateResult, WebuiFault> {
    let mut changes = std::collections::BTreeMap::new();
    for (key, value) in &request.changes {
        changes.insert(key.clone(), to_edit_value(key, value)?);
    }
    let document = open_document(services).await?;
    // Subscribed before the write, so the reload triggered below cannot slip
    // between the write and the wait.
    let mut reloads = services.config.subscribe();
    reloads.mark_unchanged();
    document
        .apply(&request.version, &changes, is_ui_editable)
        .await
        .map_err(|error| fault_from_edit(error, actor))?;
    let keys: Vec<String> = request.changes.keys().cloned().collect();
    tracing::info!(?keys, actor, "configuration edited through the webui");
    finish_edit(services, actor, &keys, &mut reloads).await
}

/// JSON to the typed edit shape: booleans, integers, strings, arrays of
/// strings, and `null` to clear. Anything else is refused here, before the
/// schema even looks at it.
fn to_edit_value(key: &str, value: &serde_json::Value) -> Result<EditValue, WebuiFault> {
    match value {
        serde_json::Value::Bool(value) => Ok(EditValue::Bool(*value)),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(EditValue::Integer)
            .ok_or_else(|| WebuiFault::Rejected(format!("{key} must be an integer"))),
        serde_json::Value::String(value) => Ok(EditValue::String(value.clone())),
        serde_json::Value::Null => Ok(EditValue::Clear),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(EditValue::Array)
            .ok_or_else(|| WebuiFault::Rejected(format!("{key} must be an array of strings"))),
        serde_json::Value::Object(_) => {
            Err(WebuiFault::Rejected(format!("{key} must be a scalar")))
        }
    }
}
