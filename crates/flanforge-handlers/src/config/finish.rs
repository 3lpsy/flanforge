use std::{sync::Arc, time::Duration};

use flanforge_config::{ConfigEditError, EditableDocument};
use flanforge_core::Config;
use flanforge_store::{Event, EventKind};
use flanforge_wire::ConfigUpdateResult;
use tokio::sync::watch;

use crate::{WebuiFault, WebuiServices};

/// How long an edit waits for the daemon's reload to confirm before answering
/// anyway; the file watcher applies the edit within seconds regardless.
const RELOAD_WAIT: Duration = Duration::from_secs(3);

/// The shared tail of every configuration write: audit event, reload nudge, a
/// bounded wait for the generation bump, and the fresh version token.
pub(super) async fn finish_edit(
    services: &WebuiServices,
    actor: &str,
    keys: &[String],
    reloads: &mut watch::Receiver<Arc<Config>>,
) -> Result<ConfigUpdateResult, WebuiFault> {
    services
        .events
        .record(
            Event::new(EventKind::ConfigEdited)
                .with_actor(actor)
                .with_payload(&serde_json::json!({ "keys": keys })),
        )
        .await;
    services.reload_now.notify_one();
    let reloaded = tokio::time::timeout(RELOAD_WAIT, reloads.changed())
        .await
        .is_ok_and(|changed| changed.is_ok());
    let document = EditableDocument::open(&services.config_path)
        .await
        .map_err(|_| WebuiFault::Internal)?;
    Ok(ConfigUpdateResult {
        version: document.version(),
        restart_pending: super::get::pending(services),
        reloaded,
    })
}

/// One mapping for every edit refusal, so a rejected profile write and a
/// rejected key edit read the same way.
pub(super) fn fault_from_edit(error: ConfigEditError, actor: &str) -> WebuiFault {
    match error {
        ConfigEditError::VersionConflict => {
            WebuiFault::Conflict("the configuration changed underneath this edit")
        }
        ConfigEditError::NotEditable { .. }
        | ConfigEditError::WrongShape { .. }
        | ConfigEditError::ProfileExists { .. }
        | ConfigEditError::ProfileMissing { .. }
        | ConfigEditError::ProfileShape { .. } => WebuiFault::Rejected(error.to_string()),
        ConfigEditError::Load(flanforge_config::ConfigLoadError::LockUnwritable) => {
            WebuiFault::Rejected(
                "the configuration file is read-only here; it is managed outside the daemon"
                    .to_owned(),
            )
        }
        ConfigEditError::Load(load) => {
            tracing::warn!(%load, actor, "webui configuration edit rejected");
            WebuiFault::Rejected(load.to_string())
        }
    }
}

/// Opens the locked document. A read-only deployment is a refusal the caller
/// can show, not an internal error.
pub(super) async fn open_document(
    services: &WebuiServices,
) -> Result<EditableDocument, WebuiFault> {
    EditableDocument::open(&services.config_path)
        .await
        .map_err(|error| match error {
            flanforge_config::ConfigLoadError::LockUnwritable => WebuiFault::Rejected(
                "the configuration file is read-only here; it is managed outside the daemon"
                    .to_owned(),
            ),
            error => {
                tracing::error!(%error, "cannot open the configuration for the webui");
                WebuiFault::Internal
            }
        })
}
