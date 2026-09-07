use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use flanforge_handlers::WebuiServices;
use flanforge_webui_auth::{oidc::OIDC_STATE_COOKIE_NAME, session_cookie};
use serde::Deserialize;

use crate::fault_response;

/// These two endpoints are the one sanctioned redirect exception on
/// `/api/v1`: the browser itself navigates them, so a redirect is the answer.
const AFTER_LOGIN: &str = "/ui/";
const AFTER_FAILURE: &str = "/ui/login?error=oidc";

fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

/// The state rides `SameSite=Lax`: the provider's redirect back is a
/// cross-site navigation, which Strict would strip the cookie from.
fn state_cookie(value: &str, is_https: bool) -> String {
    let secure = if is_https { "; Secure" } else { "" };
    format!(
        "{OIDC_STATE_COOKIE_NAME}={value}; Path=/api/v1/oidc; HttpOnly; SameSite=Lax; \
         Max-Age=600{secure}"
    )
}

fn clearing_state_cookie() -> String {
    format!("{OIDC_STATE_COOKIE_NAME}=; Path=/api/v1/oidc; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Sends the browser to the provider, carrying `state` in a bound cookie.
pub async fn login(State(services): State<WebuiServices>, headers: HeaderMap) -> Response {
    match flanforge_handlers::session::oidc::begin(&services).await {
        Ok(redirect) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, redirect.url.to_string()),
                (
                    header::SET_COOKIE,
                    state_cookie(&redirect.state, is_https(&headers)),
                ),
            ],
        )
            .into_response(),
        Err(fault) => fault_response(&fault),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

/// The provider's redirect back: on success the browser lands signed in; on
/// any broken leg it lands on the login page with a retry hint.
pub async fn callback(
    State(services): State<WebuiServices>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let state_cookie_value = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(read_state_cookie)
        .unwrap_or_default();
    match flanforge_handlers::session::oidc::finish(
        &services,
        &query.code,
        &query.state,
        &state_cookie_value,
    )
    .await
    {
        Ok(issued) => {
            login_succeeded_response(&issued.token.token, issued.ttl_seconds, is_https(&headers))
        }
        Err(fault) if fault == flanforge_handlers::WebuiFault::NotFound => fault_response(&fault),
        Err(_) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, AFTER_FAILURE.to_owned()),
                (header::SET_COOKIE, clearing_state_cookie()),
            ],
        )
            .into_response(),
    }
}

/// `AppendHeaders`, not the tuple array: the array form inserts, and a second
/// Set-Cookie would silently replace the session cookie the login just minted.
fn login_succeeded_response(token: &str, ttl_seconds: u64, is_https: bool) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, AFTER_LOGIN.to_owned())],
        axum::response::AppendHeaders([
            (
                header::SET_COOKIE,
                session_cookie(token, ttl_seconds, is_https),
            ),
            (header::SET_COOKIE, clearing_state_cookie()),
        ]),
    )
        .into_response()
}

fn read_state_cookie(cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == OIDC_STATE_COOKIE_NAME && !value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The success redirect must carry both cookies: the minted session and
    /// the state clear. An insert-style header write loses the first one.
    #[test]
    fn the_login_redirect_sets_the_session_and_clears_the_state() {
        let response = login_succeeded_response("token-1", 60, true);
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookies: Vec<&str> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        assert_eq!(cookies.len(), 2, "{cookies:?}");
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("flanforge_session=token-1"))
        );
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("flanforge_oidc_state=;"))
        );
    }
}
