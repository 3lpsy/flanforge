use std::{path::PathBuf, sync::Arc};

use flanforge_manager::{AllocationManager, ConfigHandle};
use flanforge_orm::{HistoryService, SessionService, UserService};
use flanforge_store::{BroadcastEventSink, EventSink};
use flanforge_webui_auth::oidc::OidcRelyingParty;

/// Everything a web UI action may touch. Configuration is read through the
/// reloadable handle, so `webui.public_read_only` and the session TTL follow
/// the file without a restart.
#[derive(Clone)]
pub struct WebuiServices {
    pub users: UserService,
    pub sessions: SessionService,
    pub history: HistoryService,
    pub manager: AllocationManager,
    pub config: Arc<ConfigHandle>,
    /// The live event feed the SSE endpoint subscribes to.
    pub event_feed: Arc<BroadcastEventSink>,
    /// Present iff `[webui.oidc]` is enabled and its secret was readable.
    pub oidc: Option<Arc<OidcRelyingParty>>,
    /// The durable event sink the daemon composes; config edits land here.
    pub events: Arc<dyn EventSink>,
    /// Where the configuration file lives; the edit endpoints write it.
    pub config_path: PathBuf,
    /// Nudges the daemon's reload watcher after an edit lands.
    pub reload_now: Arc<tokio::sync::Notify>,
    pub version: String,
}

impl std::fmt::Debug for WebuiServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebuiServices")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}
