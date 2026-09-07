use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

/// Binds the browser that started a login to the one that finishes it,
/// `SameSite=Lax` because the provider's redirect back is cross-site.
pub const OIDC_STATE_COOKIE_NAME: &str = "flanforge_oidc_state";

const TICKET_TTL: Duration = Duration::from_mins(10);
const MAX_PENDING: usize = 64;

/// One login in flight: minted at the authorize leg, consumed exactly once at
/// the callback. Held in memory — a restart aborts the login, which is fine.
#[derive(Clone, Debug)]
pub(super) struct PendingLogin {
    pub nonce: String,
    pub pkce_verifier: String,
    started: Instant,
}

#[derive(Debug, Default)]
pub(super) struct PendingLogins {
    inner: Mutex<HashMap<String, PendingLogin>>,
}

impl PendingLogins {
    /// Registers a fresh state's login; the map is bounded, oldest evicted.
    pub(super) fn insert(&self, state: &str, nonce: String, pkce_verifier: String) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.retain(|_, pending| pending.started.elapsed() < TICKET_TTL);
        if inner.len() >= MAX_PENDING
            && let Some(oldest) = inner
                .iter()
                .min_by_key(|(_, pending)| pending.started)
                .map(|(key, _)| key.clone())
        {
            inner.remove(&oldest);
        }
        inner.insert(
            key(state),
            PendingLogin {
                nonce,
                pkce_verifier,
                started: Instant::now(),
            },
        );
    }

    /// Single use: a replayed state finds nothing the second time.
    pub(super) fn take(&self, state: &str) -> Option<PendingLogin> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .remove(&key(state))
            .filter(|pending| pending.started.elapsed() < TICKET_TTL)
    }
}

/// Keyed by hash so the map never holds a value equal to the cookie.
fn key(state: &str) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(state.as_bytes()) {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
