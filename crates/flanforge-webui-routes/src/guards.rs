use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, Uri, header},
    middleware::Next,
    response::Response,
};
use flanforge_extractors::{WebuiIdentity, error_response};
use flanforge_handlers::WebuiServices;

/// Required on every non-GET `/api/v1` request. A cross-site attacker cannot
/// set a custom header without a CORS preflight this server never answers.
pub const CSRF_HEADER: &str = "x-flanforge-webui";

fn is_signed_in(request: &Request) -> bool {
    request
        .extensions()
        .get::<WebuiIdentity>()
        .is_some_and(|identity| identity.0.is_some())
}

/// Guard for the readonly tier: signed in, or anyone when
/// `webui.public_read_only` is on. The flag is read per request through the
/// reloadable handle, so an edit applies at the next reload tick.
pub async fn require_read(
    State(services): State<WebuiServices>,
    request: Request,
    next: Next,
) -> Response {
    if services.config.current().webui.public_read_only || is_signed_in(&request) {
        return next.run(request).await;
    }
    error_response(StatusCode::UNAUTHORIZED, "authentication required")
}

/// Guard for the mutating tier — which includes sensitive reads such as logs,
/// configuration, and accounts: signed in or 401, never a redirect.
pub async fn require_auth(request: Request, next: Next) -> Response {
    if is_signed_in(&request) {
        return next.run(request).await;
    }
    error_response(StatusCode::UNAUTHORIZED, "authentication required")
}

/// CSRF wall over the mutating tier: `SameSite=Strict` cookies are the first
/// layer, the custom header is the second, and an `Origin` that disagrees
/// with `Host` is refused outright as the third.
pub async fn ensure_csrf(request: Request, next: Next) -> Response {
    let is_read = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !is_read {
        if request.headers().get(CSRF_HEADER).is_none() {
            return error_response(StatusCode::FORBIDDEN, "missing the request header");
        }
        if !is_origin_consistent(&request) {
            return error_response(StatusCode::FORBIDDEN, "cross-origin request refused");
        }
    }
    next.run(request).await
}

/// True when no `Origin` is present, or its host equals the request `Host`.
fn is_origin_consistent(request: &Request) -> bool {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(origin_authority) = origin
        .parse::<Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(std::string::ToString::to_string))
    else {
        return false;
    };
    request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| host == origin_authority)
}
