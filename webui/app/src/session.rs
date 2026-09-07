//! The bootstrap-session context: one `GET /api/v1/meta` drives the navbar,
//! auth-aware widgets, and the login-page redirect.

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_wire::WebuiMeta;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Loading,
    Ready(WebuiMeta),
    Error(String),
}

#[derive(Clone, Copy)]
pub struct SessionCtx {
    pub state: Signal<SessionState>,
}

impl SessionCtx {
    /// The current meta document, if loaded.
    pub fn meta(&self) -> Option<WebuiMeta> {
        match &*self.state.read() {
            SessionState::Ready(meta) => Some(meta.clone()),
            _ => None,
        }
    }

    /// Whether the caller is signed in.
    pub fn is_signed_in(&self) -> bool {
        self.meta().is_some_and(|meta| meta.user.is_some())
    }

    /// Whether anonymous state views are open.
    pub fn is_public(&self) -> bool {
        self.meta().is_some_and(|meta| meta.public_read_only)
    }

    /// Re-fetches meta (after logout, account edits). `spawn_forever`
    /// survives the caller unmounting, so the navbar never keeps stale state.
    pub fn refresh(&self) {
        let this = *self;
        spawn_forever(async move {
            this.refresh_now().await;
        });
    }

    /// Refreshes and *waits*. Login uses this before navigating, so route
    /// guards and the navbar see the new identity, not the stale meta.
    pub async fn refresh_now(&self) {
        let mut state = self.state;
        match api::get_json::<WebuiMeta>("/api/v1/meta").await {
            Ok(meta) => state.set(SessionState::Ready(meta)),
            Err(error) => state.set(SessionState::Error(error.to_string())),
        }
    }
}

/// Provides the context and kicks off the initial fetch. Call once, in `app`.
pub fn provide() -> SessionCtx {
    let state = use_signal(|| SessionState::Loading);
    let ctx = use_context_provider(|| SessionCtx { state });
    use_effect(move || ctx.refresh());
    ctx
}

pub fn use_session() -> SessionCtx {
    use_context::<SessionCtx>()
}
