use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use flanforge_core::{ForgejoClaims, OidcConfig};
use http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use url::Url;

use super::{AuthError, OidcVerifier, TokenVerifier, bearer_token};

#[test]
fn accepts_one_strict_bearer_token() {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer one.two.three"),
    );
    assert_eq!(bearer_token(&headers), Ok("one.two.three"));
}

#[test]
fn rejects_other_schemes_and_embedded_whitespace() {
    for value in [
        "bearer token",
        "Basic token",
        "Bearer two words",
        "Bearer one.two",
        "Bearer one.two.three.four",
        "Bearer one.two.$(id)",
        "Bearer ",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(value).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        assert_eq!(bearer_token(&headers), Err(AuthError::InvalidToken));
    }
}

#[test]
fn rejects_duplicate_headers() {
    let mut headers = HeaderMap::new();
    headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer first"));
    headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer second"));
    assert_eq!(bearer_token(&headers), Err(AuthError::InvalidToken));
}

#[tokio::test]
async fn verifies_signature_issuer_audience_and_time_claims() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let application = Router::new().route("/jwks", get(jwks));
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });
    let issuer = format!("http://{address}/api/actions");
    let verifier = OidcVerifier::new(Arc::new(OidcConfig {
        issuer: Url::parse(&issuer).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        audience: "flanforged".into(),
        jwks_url: Url::parse(&format!("http://{address}/jwks"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jwks_cache_seconds: 300,
        clock_skew_seconds: 30,
    }))
    .unwrap_or_else(|error| unreachable!("verifier: {error}"));

    let token = signed_token(&issuer, "flanforged");
    let claims = verifier
        .verify(&token)
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));
    assert_eq!(claims.repository, "owner/project");

    let wrong_audience = signed_token(&issuer, "somewhere-else");
    assert_eq!(
        verifier.verify(&wrong_audience).await,
        Err(AuthError::InvalidToken)
    );
}

#[tokio::test]
async fn unknown_key_ids_share_one_refresh_during_the_cooldown() {
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let application = Router::new()
        .route("/jwks", get(counted_jwks))
        .with_state(requests.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });
    let issuer = format!("http://{address}/api/actions");
    let verifier = OidcVerifier::new(Arc::new(OidcConfig {
        issuer: Url::parse(&issuer).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        audience: "flanforged".into(),
        jwks_url: Url::parse(&format!("http://{address}/jwks"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jwks_cache_seconds: 300,
        clock_skew_seconds: 30,
    }))
    .unwrap_or_else(|error| unreachable!("verifier: {error}"));
    let token = signed_token_with_kid(&issuer, "flanforged", "unknown-key");

    let (first, second, third) = tokio::join!(
        verifier.verify(&token),
        verifier.verify(&token),
        verifier.verify(&token),
    );
    assert_eq!(first, Err(AuthError::InvalidToken));
    assert_eq!(second, Err(AuthError::InvalidToken));
    assert_eq!(third, Err(AuthError::InvalidToken));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_keys_are_not_served_while_the_refresh_cooldown_holds() {
    let failing = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let application = Router::new()
        .route("/jwks", get(failing_jwks))
        .with_state(failing.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });
    let issuer = format!("http://{address}/api/actions");
    let verifier = OidcVerifier::new(Arc::new(OidcConfig {
        issuer: Url::parse(&issuer).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        audience: "flanforged".into(),
        jwks_url: Url::parse(&format!("http://{address}/jwks"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jwks_cache_seconds: 300,
        clock_skew_seconds: 30,
    }))
    .unwrap_or_else(|error| unreachable!("verifier: {error}"));
    let token = signed_token(&issuer, "flanforged");
    verifier
        .verify(&token)
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));

    failing.store(true, Ordering::SeqCst);
    verifier
        .age_cache_for_tests(Duration::from_secs(400), Duration::from_secs(400))
        .await;
    assert_eq!(verifier.verify(&token).await, Err(AuthError::Unavailable));
    assert_eq!(verifier.verify(&token).await, Err(AuthError::Unavailable));
}

#[tokio::test]
async fn forced_refresh_in_the_cooldown_still_uses_keys_within_their_lifetime() {
    let failing = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let application = Router::new()
        .route("/jwks", get(failing_jwks))
        .with_state(failing.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });
    let issuer = format!("http://{address}/api/actions");
    let verifier = OidcVerifier::new(Arc::new(OidcConfig {
        issuer: Url::parse(&issuer).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        audience: "flanforged".into(),
        jwks_url: Url::parse(&format!("http://{address}/jwks"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jwks_cache_seconds: 300,
        clock_skew_seconds: 30,
    }))
    .unwrap_or_else(|error| unreachable!("verifier: {error}"));
    verifier
        .verify(&signed_token(&issuer, "flanforged"))
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));

    failing.store(true, Ordering::SeqCst);
    verifier
        .age_cache_for_tests(Duration::from_mins(1), Duration::from_secs(0))
        .await;
    let unknown = signed_token_with_kid(&issuer, "flanforged", "unknown-key");
    assert_eq!(
        verifier.verify(&unknown).await,
        Err(AuthError::InvalidToken)
    );
}

async fn failing_jwks(State(failing): State<Arc<AtomicBool>>) -> Result<Json<Value>, StatusCode> {
    if failing.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(jwks().await)
}

async fn jwks() -> Json<Value> {
    Json(json!({"keys": [{
        "kty": "RSA",
        "n": "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ",
        "e": "AQAB", "kid": "rsa01", "alg": "RS256", "use": "sig"
    }]}))
}

async fn counted_jwks(State(requests): State<Arc<AtomicUsize>>) -> Json<Value> {
    requests.fetch_add(1, Ordering::SeqCst);
    jwks().await
}

fn signed_token(issuer: &str, audience: &str) -> String {
    signed_token_with_kid(issuer, audience, "rsa01")
}

fn signed_token_with_kid(issuer: &str, audience: &str, key_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| unreachable!("clock: {error}"))
        .as_secs();
    let claims = ForgejoClaims {
        actor: "developer".into(),
        aud: audience.into(),
        event_name: "push".into(),
        exp: now + 300,
        iat: now,
        iss: issuer.into(),
        nbf: now.saturating_sub(1),
        git_ref: "refs/heads/main".into(),
        ref_protected: "true".into(),
        ref_type: "branch".into(),
        repository: "owner/project".into(),
        repository_owner: "owner".into(),
        run_attempt: "1".into(),
        run_id: "42".into(),
        run_number: "7".into(),
        sha: "0123456789012345678901234567890123456789".into(),
        sub: "repo:owner/project:ref:refs/heads/main".into(),
        workflow: "apple.yml".into(),
        workflow_ref: "owner/project/.forgejo/workflows/apple.yml@refs/heads/main".into(),
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key_id.into());
    let key = EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/rsa-private.pem"))
        .unwrap_or_else(|error| unreachable!("fixture key: {error}"));
    encode(&header, &claims, &key).unwrap_or_else(|error| unreachable!("token: {error}"))
}
