use axum::{Router, body::Body as HttpBody, http::Request, routing::post};
use tower::util::ServiceExt;

use super::*;

async fn status_for(json: &str) -> axum::http::StatusCode {
    let app: Router = Router::new().route(
        "/login",
        post(
            |Body(request): Body<flanforge_wire::LoginRequest>| async move {
                drop(request);
                "ok"
            },
        ),
    );
    let response = app
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/json")
                .body(HttpBody::from(json.to_owned()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    response.status()
}

#[tokio::test]
async fn a_body_that_fails_validation_never_reaches_the_handler() {
    assert_eq!(
        status_for(r#"{"username":"jim","password":"pw"}"#).await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        status_for(r#"{"username":"","password":"pw"}"#).await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for(r#"{"username":"jim","password":"pw","extra":1}"#).await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for("not json").await,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn identity_extractors_fail_closed_without_the_middleware() {
    let app: Router = Router::new().route(
        "/me",
        post(|RequireUser(resolved): RequireUser| async move { resolved.session.user.username }),
    );
    let response = app
        .oneshot(
            Request::post("/me")
                .body(HttpBody::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}
