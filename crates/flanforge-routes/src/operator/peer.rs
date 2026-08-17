use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::header::{HOST, ORIGIN},
    middleware::Next,
    response::Response,
};
use flanforge_manager::{OPERATOR_TOKEN_HEADER, is_token_match};

use super::{super::error::ApiError, OperatorState};

/// Admits only a request that could have come from a shell on this host: a
/// local peer, no browser origin, the bound authority, and the credential the
/// daemon wrote 0600 into its state directory.
///
/// The peer address alone is not identity. A forwarder re-originates every
/// remote connection from loopback, and a page in a browser is loopback too;
/// only the credential distinguishes them from the operator.
///
/// # Errors
///
/// Returns a forbidden rejection for every other request.
pub async fn ensure_local_peer(
    State(state): State<OperatorState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let refusal = if !(peer.ip().is_loopback() || peer.ip() == state.listen.ip()) {
        Some("peer is not local")
    } else if request.headers().contains_key(ORIGIN) {
        Some("request carries a browser origin")
    } else if !is_bound_authority(&request, state.listen) {
        Some("Host does not name the bound loopback authority")
    } else if !is_authorized(&request, &state.token) {
        Some("host-only operator credential is absent or wrong")
    } else {
        None
    };
    match refusal {
        None => Ok(next.run(request).await),
        Some(reason) => {
            tracing::warn!(%peer, path = %request.uri().path(), reason, "operator request rejected");
            Err(ApiError::Forbidden)
        }
    }
}

/// A name that resolves to loopback is not the loopback address: a rebound DNS
/// entry sends its own name here, which never parses as a socket address.
fn is_bound_authority(request: &Request, listen: SocketAddr) -> bool {
    request
        .headers()
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse::<SocketAddr>().ok())
        .is_some_and(|address| {
            address.port() == listen.port()
                && (address.ip().is_loopback() || address.ip() == listen.ip())
        })
}

fn is_authorized(request: &Request, token: &str) -> bool {
    request
        .headers()
        .get(OPERATOR_TOKEN_HEADER)
        .and_then(|presented| presented.to_str().ok())
        .is_some_and(|presented| is_token_match(presented, token))
}
