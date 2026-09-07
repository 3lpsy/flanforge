use flanforge_orm::SessionRecord;
use flanforge_wire::{WebuiMeta, WebuiSessionUser};

use crate::WebuiServices;

/// The SPA's bootstrap document. Anonymous callers on a private deployment
/// get only what the login page needs — no version, no backend.
pub async fn handle(services: &WebuiServices, identity: Option<&SessionRecord>) -> WebuiMeta {
    let config = services.config.current();
    let webui = &config.webui;
    let is_visible = identity.is_some() || webui.public_read_only;
    // A count failure must not take the login page down with it; the hint is
    // best-effort and the form still works.
    let needs_bootstrap = webui.authdb.enabled
        && services.users.count().await.map_or_else(
            |error| {
                tracing::error!(%error, "cannot count webui users");
                false
            },
            |count| count == 0,
        );
    WebuiMeta {
        version: if is_visible {
            services.version.clone()
        } else {
            String::new()
        },
        backend: if is_visible {
            config.runtime.backend_kind().to_string()
        } else {
            String::new()
        },
        authdb_enabled: webui.authdb.enabled,
        oidc_enabled: webui.oidc.enabled,
        public_read_only: webui.public_read_only,
        needs_bootstrap,
        user: identity.map(|session| WebuiSessionUser {
            id: session.user.id,
            username: session.user.username.clone(),
            auth_source: session.user.auth_source.as_str().to_owned(),
        }),
    }
}
