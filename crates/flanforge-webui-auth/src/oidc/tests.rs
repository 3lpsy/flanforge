use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::get, routing::post};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use url::Url;

use super::*;

const TEST_PEM: &str = include_str!("test_key.pem");
const TEST_JWKS: &str = include_str!("test_jwks.json");
const CLIENT_ID: &str = "flanforge-webui";

#[derive(Clone, Debug, Serialize)]
struct Claims {
    iss: String,
    aud: String,
    sub: String,
    iat: u64,
    exp: u64,
    nonce: String,
    preferred_username: String,
}

#[derive(Clone)]
struct FakeIdp {
    issuer: Arc<Mutex<String>>,
    /// The nonce the next minted id token carries; the test copies it from
    /// the authorize URL, standing in for the browser leg.
    nonce: Arc<Mutex<String>>,
}

async fn spawn_idp() -> (FakeIdp, Url) {
    let idp = FakeIdp {
        issuer: Arc::new(Mutex::new(String::new())),
        nonce: Arc::new(Mutex::new(String::new())),
    };
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(|State(idp): State<FakeIdp>| async move {
                let issuer = idp
                    .issuer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Json(serde_json::json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "jwks_uri": format!("{issuer}/jwks"),
                }))
            }),
        )
        .route(
            "/jwks",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    TEST_JWKS,
                )
            }),
        )
        .route(
            "/token",
            post(|State(idp): State<FakeIdp>| async move {
                let issuer = idp
                    .issuer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let nonce = idp
                    .nonce
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let claims = Claims {
                    iss: issuer,
                    aud: CLIENT_ID.to_owned(),
                    sub: "subject-1".to_owned(),
                    iat: now,
                    exp: now + 300,
                    nonce,
                    preferred_username: "jim".to_owned(),
                };
                let mut header = Header::new(Algorithm::RS256);
                header.kid = Some("test-key".to_owned());
                let key = EncodingKey::from_rsa_pem(TEST_PEM.as_bytes())
                    .unwrap_or_else(|error| unreachable!("fixture key: {error}"));
                let id_token = jsonwebtoken::encode(&header, &claims, &key)
                    .unwrap_or_else(|error| unreachable!("sign: {error}"));
                Json(serde_json::json!({ "id_token": id_token }))
            }),
        )
        .with_state(idp.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("addr: {error}"));
    let issuer = format!("http://{address}");
    *idp.issuer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = issuer.clone();
    tokio::spawn(async move {
        drop(axum::serve(listener, app).await);
    });
    (
        idp,
        issuer
            .parse()
            .unwrap_or_else(|error| unreachable!("issuer: {error}")),
    )
}

fn relying_party(issuer: Url) -> OidcRelyingParty {
    OidcRelyingParty::new(OidcRpConfig {
        issuer,
        client_id: CLIENT_ID.to_owned(),
        client_secret: "shh".to_owned(),
        redirect_url: "http://127.0.0.1:9843/api/v1/oidc/callback"
            .parse()
            .unwrap_or_else(|error| unreachable!("{error}")),
        scopes: vec!["openid".to_owned(), "profile".to_owned()],
    })
    .unwrap_or_else(|error| unreachable!("rp: {error}"))
}

fn query_value(url: &Url, name: &str) -> String {
    url.query_pairs().find(|(key, _)| key == name).map_or_else(
        || unreachable!("{name} in authorize URL"),
        |(_, value)| value.into_owned(),
    )
}

#[tokio::test]
async fn the_full_code_flow_verifies_state_pkce_and_nonce() {
    let (idp, issuer) = spawn_idp().await;
    let rp = relying_party(issuer);

    let redirect = rp
        .begin_login()
        .await
        .unwrap_or_else(|error| unreachable!("begin: {error}"));
    assert_eq!(query_value(&redirect.url, "response_type"), "code");
    assert_eq!(query_value(&redirect.url, "code_challenge_method"), "S256");
    assert_eq!(query_value(&redirect.url, "state"), redirect.state);
    *idp.nonce
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = query_value(&redirect.url, "nonce");

    let identity = rp
        .finish_login("the-code", &redirect.state, &redirect.state)
        .await
        .unwrap_or_else(|error| unreachable!("finish: {error}"));
    assert_eq!(identity.subject, "subject-1");
    assert_eq!(identity.preferred_username, "jim");

    // The state is single-use: a replay finds no ticket.
    assert_eq!(
        rp.finish_login("the-code", &redirect.state, &redirect.state)
            .await,
        Err(OidcError::UnknownTicket)
    );
}

#[tokio::test]
async fn a_state_that_does_not_match_the_cookie_is_refused() {
    let (_idp, issuer) = spawn_idp().await;
    let rp = relying_party(issuer);
    let redirect = rp
        .begin_login()
        .await
        .unwrap_or_else(|error| unreachable!("begin: {error}"));
    assert_eq!(
        rp.finish_login("the-code", &redirect.state, "someone-elses-state")
            .await,
        Err(OidcError::UnknownTicket)
    );
    assert_eq!(
        rp.finish_login("the-code", "forged-state", "forged-state")
            .await,
        Err(OidcError::UnknownTicket)
    );
}

#[tokio::test]
async fn a_wrong_nonce_in_the_id_token_is_refused() {
    let (idp, issuer) = spawn_idp().await;
    let rp = relying_party(issuer);
    let redirect = rp
        .begin_login()
        .await
        .unwrap_or_else(|error| unreachable!("begin: {error}"));
    *idp.nonce
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = "a-different-nonce".to_owned();
    assert_eq!(
        rp.finish_login("the-code", &redirect.state, &redirect.state)
            .await,
        Err(OidcError::Token("nonce"))
    );
}
