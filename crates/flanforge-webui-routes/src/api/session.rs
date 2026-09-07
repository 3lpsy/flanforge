use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use flanforge_extractors::Body;
use flanforge_handlers::WebuiServices;
use flanforge_webui_auth::{clearing_cookie, cookie_token_hashes, session_cookie};
use flanforge_wire::LoginRequest;

use crate::fault_response;

/// Whether the request provably arrived over HTTPS; decides the cookie's
/// `Secure` attribute without breaking plain-HTTP LAN deployments.
fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

/// Password login: 204 with the session cookie, or the uniform refusal.
pub async fn login(
    State(services): State<WebuiServices>,
    headers: HeaderMap,
    Body(request): Body<LoginRequest>,
) -> Response {
    match flanforge_handlers::session::login(&services, &request).await {
        Ok(issued) => {
            let cookie =
                session_cookie(&issued.token.token, issued.ttl_seconds, is_https(&headers));
            ([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response()
        }
        Err(fault) => fault_response(&fault),
    }
}

/// Logout: retires every presented session and clears the cookie. Idempotent.
pub async fn logout(State(services): State<WebuiServices>, headers: HeaderMap) -> Response {
    let token_hashes = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(cookie_token_hashes)
        .unwrap_or_default();
    match flanforge_handlers::session::logout(&services, &token_hashes).await {
        Ok(()) => (
            [(header::SET_COOKIE, clearing_cookie())],
            StatusCode::NO_CONTENT,
        )
            .into_response(),
        Err(fault) => fault_response(&fault),
    }
}
