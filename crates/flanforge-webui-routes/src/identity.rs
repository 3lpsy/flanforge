use axum::{
    extract::{Request, State},
    http::header::COOKIE,
    middleware::Next,
    response::Response,
};
use flanforge_extractors::{ResolvedSession, WebuiIdentity};
use flanforge_handlers::WebuiServices;
use flanforge_webui_auth::cookie_token_hashes;

/// Resolves the caller's session and attaches it. Applied over the whole
/// `/api/v1` branch — never over `/ui` assets, so a static file costs no
/// database read. Rejection is the tier guards' job, not this one's.
pub async fn attach_identity(
    State(services): State<WebuiServices>,
    mut request: Request,
    next: Next,
) -> Response {
    // Only the header crosses the await: the request body is not `Sync`.
    let cookie_header = request
        .headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let resolved = resolve(&services, cookie_header.as_deref()).await;
    request.extensions_mut().insert(WebuiIdentity(resolved));
    next.run(request).await
}

/// Every presented `flanforge_session` value is checked: browsers may send
/// stale duplicates set under other paths. A store failure reads as signed
/// out, never as signed in.
async fn resolve(services: &WebuiServices, cookie_header: Option<&str>) -> Option<ResolvedSession> {
    let cookie_header = cookie_header?;
    for token_hash in cookie_token_hashes(cookie_header) {
        match services.sessions.resolve(&token_hash).await {
            Ok(Some(session)) => {
                return Some(ResolvedSession {
                    session,
                    token_hash,
                });
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "webui session resolution failed");
                return None;
            }
        }
    }
    None
}
