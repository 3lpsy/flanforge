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

use super::{
    AuthError, OidcVerifier, ProviderFailure, TokenRejection, TokenVerifier, bearer_token,
};

#[test]
fn accepts_one_strict_bearer_token() {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer one.two.three"),
    );
    assert_eq!(bearer_token(&headers), Ok("one.two.three"));
}

// A wrong scheme, a wrong shape, and an over-long value are three different
// operator problems, so each names its own reason (ARCH-613).
#[test]
fn rejects_other_schemes_and_embedded_whitespace() {
    for (value, reason) in [
        ("bearer token", TokenRejection::Scheme),
        ("Basic token", TokenRejection::Scheme),
        ("Bearer two words", TokenRejection::CompactShape),
        ("Bearer one.two", TokenRejection::CompactShape),
        ("Bearer one.two.three.four", TokenRejection::CompactShape),
        ("Bearer one.two.$(id)", TokenRejection::CompactShape),
        ("Bearer ", TokenRejection::CompactShape),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(value).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        );
        assert_eq!(
            bearer_token(&headers),
            Err(AuthError::InvalidToken(reason)),
            "{value}"
        );
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", "a".repeat(16_385)))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    assert_eq!(
        bearer_token(&headers),
        Err(AuthError::InvalidToken(TokenRejection::Oversize))
    );
}

#[test]
fn rejects_duplicate_headers() {
    let mut headers = HeaderMap::new();
    headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer first"));
    headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer second"));
    assert_eq!(
        bearer_token(&headers),
        Err(AuthError::InvalidToken(TokenRejection::DuplicateHeader))
    );
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
        Err(AuthError::InvalidToken(TokenRejection::Audience))
    );

    // The header is read before any key work, so a non-RS256 algorithm and an
    // absent key ID are distinguishable from a bad signature.
    assert_eq!(
        verifier.verify("not.a.token").await,
        Err(AuthError::InvalidToken(TokenRejection::Header))
    );
    assert_eq!(
        verifier
            .verify(&sign_without_kid(&token_claims(&issuer, "flanforged")))
            .await,
        Err(AuthError::InvalidToken(TokenRejection::KeyId))
    );
    assert_eq!(
        verifier.verify(&"a".repeat(16_385)).await,
        Err(AuthError::InvalidToken(TokenRejection::Oversize))
    );

    // Time claims each name their own check rather than one bare rejection.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| unreachable!("clock: {error}"))
        .as_secs();
    let mut expired = token_claims(&issuer, "flanforged");
    expired.iat = now - 600;
    expired.nbf = now - 600;
    expired.exp = now - 300;
    assert_eq!(
        verifier.verify(&sign(&expired, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::Expired))
    );

    let mut immature = token_claims(&issuer, "flanforged");
    immature.nbf = now + 600;
    immature.exp = now + 900;
    assert_eq!(
        verifier.verify(&sign(&immature, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::NotYetValid))
    );

    let mut long_lived = token_claims(&issuer, "flanforged");
    long_lived.exp = long_lived.iat + 7_201;
    assert_eq!(
        verifier.verify(&sign(&long_lived, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::LifetimeCap))
    );

    let mut future = token_claims(&issuer, "flanforged");
    future.iat = now + 600;
    future.exp = future.iat + 300;
    assert_eq!(
        verifier.verify(&sign(&future, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::IssuedInFuture))
    );
}

// The incident this reason exists for: a claim that fails its own shape check
// must name the claim and the check, not collapse into "invalid token".
#[tokio::test]
async fn a_claim_shape_rejection_names_the_claim_and_the_check() {
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

    for (mutate, field, code) in [
        (
            (|claims: &mut ForgejoClaims| claims.ref_type = "b".repeat(33))
                as fn(&mut ForgejoClaims),
            "ref_type",
            "length",
        ),
        (|claims| claims.sha = "abc".into(), "sha", "sha"),
        (
            |claims| claims.repository = "owner".into(),
            "repository",
            "repository",
        ),
        (
            |claims| claims.ref_protected = "yes".into(),
            "ref_protected",
            "boolean",
        ),
        (
            |claims| claims.run_id = "0".into(),
            "run_id",
            "positive_decimal",
        ),
        (|claims| claims.actor = "a".repeat(101), "actor", "length"),
    ] {
        let mut claims = token_claims(&issuer, "flanforged");
        mutate(&mut claims);
        assert_eq!(
            verifier.verify(&sign(&claims, "rsa01")).await,
            Err(AuthError::InvalidToken(TokenRejection::ClaimShape {
                field,
                code
            })),
            "{field}"
        );
    }
}

// A scheduled Forgejo run signs an empty `ref_type`. Verification must accept
// it: no policy reads the claim, and rejecting it stopped every periodic
// allocation at the authentication boundary (CORE-606).
#[tokio::test]
async fn an_empty_ref_type_still_verifies() {
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

    let mut scheduled = token_claims(&issuer, "flanforged");
    scheduled.ref_type = String::new();
    let accepted = verifier
        .verify(&sign(&scheduled, "rsa01"))
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));
    assert_eq!(accepted.ref_type, "");

    let mut over_long = token_claims(&issuer, "flanforged");
    over_long.ref_type = "b".repeat(33);
    assert_eq!(
        verifier.verify(&sign(&over_long, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::ClaimShape {
            field: "ref_type",
            code: "length"
        }))
    );

    // The full scheduled shape: short ref in every ref-bearing claim, expanded
    // before validation so the token verifies at all (CORE-607).
    let mut scheduled = token_claims(&issuer, "flanforged");
    scheduled.event_name = "schedule".into();
    scheduled.ref_type = String::new();
    scheduled.git_ref = "main".into();
    scheduled.sub = "repo:owner/project:ref:main".into();
    scheduled.workflow_ref = "owner/project/.forgejo/workflows/apple.yml@main".into();
    let expanded = verifier
        .verify(&sign(&scheduled, "rsa01"))
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));
    assert_eq!(expanded.git_ref, "refs/heads/main");
    assert_eq!(expanded.sub, "repo:owner/project:ref:refs/heads/main");
    assert_eq!(
        expanded.workflow_ref,
        "owner/project/.forgejo/workflows/apple.yml@refs/heads/main"
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
    assert_eq!(
        first,
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    );
    assert_eq!(
        second,
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    );
    assert_eq!(
        third,
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    );
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
    // A refusing JWKS host and a cooldown with nothing usable cached are two
    // different operator problems, so they no longer share one reason.
    assert_eq!(
        verifier.verify(&token).await,
        Err(AuthError::Unavailable(ProviderFailure::Status))
    );
    assert_eq!(
        verifier.verify(&token).await,
        Err(AuthError::Unavailable(ProviderFailure::Cooldown))
    );
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
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    );
}

async fn failing_jwks(State(failing): State<Arc<AtomicBool>>) -> Result<Json<Value>, StatusCode> {
    if failing.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(jwks().await)
}

/// The public half of `tests/fixtures/test-key-private.pem`.
const RSA_MODULUS: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";

/// The fixture key published under one key id.
fn rsa_jwk(key_id: &str) -> Value {
    json!({
        "kty": "RSA", "n": RSA_MODULUS, "e": "AQAB",
        "kid": key_id, "alg": "RS256", "use": "sig"
    })
}

async fn jwks() -> Json<Value> {
    Json(json!({ "keys": [rsa_jwk("rsa01")] }))
}

async fn counted_jwks(State(requests): State<Arc<AtomicUsize>>) -> Json<Value> {
    requests.fetch_add(1, Ordering::SeqCst);
    jwks().await
}

fn signed_token(issuer: &str, audience: &str) -> String {
    signed_token_with_kid(issuer, audience, "rsa01")
}

fn signed_token_with_kid(issuer: &str, audience: &str, key_id: &str) -> String {
    sign(&token_claims(issuer, audience), key_id)
}

fn token_claims(issuer: &str, audience: &str) -> ForgejoClaims {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| unreachable!("clock: {error}"))
        .as_secs();
    ForgejoClaims {
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
    }
}

fn sign_without_kid(claims: &ForgejoClaims) -> String {
    let header = Header::new(Algorithm::RS256);
    let key = EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/test-key-private.pem"))
        .unwrap_or_else(|error| unreachable!("fixture key: {error}"));
    encode(&header, claims, &key).unwrap_or_else(|error| unreachable!("token: {error}"))
}

fn sign(claims: &ForgejoClaims, key_id: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key_id.into());
    let key = EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/test-key-private.pem"))
        .unwrap_or_else(|error| unreachable!("fixture key: {error}"));
    encode(&header, claims, &key).unwrap_or_else(|error| unreachable!("token: {error}"))
}

fn sign_payload(payload: &Value, key_id: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key_id.into());
    let key = EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/test-key-private.pem"))
        .unwrap_or_else(|error| unreachable!("fixture key: {error}"));
    encode(&header, payload, &key).unwrap_or_else(|error| unreachable!("token: {error}"))
}

/// Serves one JWKS router and points a verifier at it. Every negative case
/// below needs its own provider, so the setup is shared rather than repeated.
async fn verifier_for(routes: Router) -> (OidcVerifier, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, routes).await;
    });
    let issuer = format!("http://{address}/api/actions");
    (
        verifier_at(&issuer, &format!("http://{address}/jwks")),
        issuer,
    )
}

fn verifier_at(issuer: &str, jwks_url: &str) -> OidcVerifier {
    OidcVerifier::new(Arc::new(OidcConfig {
        issuer: Url::parse(issuer).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        audience: "flanforged".into(),
        jwks_url: Url::parse(jwks_url).unwrap_or_else(|error| unreachable!("fixture: {error}")),
        jwks_cache_seconds: 300,
        clock_skew_seconds: 30,
    }))
    .unwrap_or_else(|error| unreachable!("verifier: {error}"))
}

/// A verifier over one fixed JWKS body, so a case can state the identity
/// provider response it is about inline.
async fn verifier_serving(body: Value) -> (OidcVerifier, String) {
    verifier_for(
        Router::new()
            .route("/jwks", get(fixed_jwks))
            .with_state(Arc::new(body)),
    )
    .await
}

async fn fixed_jwks(State(body): State<Arc<Value>>) -> Json<Value> {
    Json(body.as_ref().clone())
}

async fn not_json() -> &'static str {
    "<html>a proxy error page</html>"
}

async fn oversized_jwks() -> Json<Value> {
    Json(json!({ "keys": [rsa_jwk("rsa01")], "padding": "x".repeat(300_000) }))
}

async fn rotating_jwks(State(rotated): State<Arc<AtomicBool>>) -> Json<Value> {
    let key_id = if rotated.load(Ordering::SeqCst) {
        "rsa02"
    } else {
        "rsa01"
    };
    Json(json!({ "keys": [rsa_jwk(key_id)] }))
}

/// Replaces the first character of one base64url segment, keeping the shape.
fn corrupt_segment(token: &str, index: usize) -> String {
    let mut segments: Vec<String> = token.split('.').map(str::to_owned).collect();
    let segment = &mut segments[index];
    let first = segment.remove(0);
    segment.insert(0, if first == 'a' { 'b' } else { 'a' });
    segments.join(".")
}

// Neither half of the binding a signature provides had a negative case: the
// suite proved a good token verifies, not that a bad one cannot.
#[tokio::test]
async fn a_tampered_token_or_a_foreign_issuer_never_verifies() {
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(jwks))).await;
    let token = signed_token(&issuer, "flanforged");

    // Rewriting the claims and keeping the signature breaks the binding, and so
    // does keeping the claims and forging the signature.
    for tampered in [corrupt_segment(&token, 1), corrupt_segment(&token, 2)] {
        assert_ne!(tampered, token);
        assert_eq!(
            verifier.verify(&tampered).await,
            Err(AuthError::InvalidToken(TokenRejection::Signature)),
            "{tampered}"
        );
    }

    // A correctly signed token from another issuer is still another issuer's.
    let mut foreign = token_claims(&issuer, "flanforged");
    foreign.iss = "https://elsewhere.example/api/actions".into();
    assert_eq!(
        verifier.verify(&sign(&foreign, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::Issuer))
    );
}

// Algorithm confusion: the header names the algorithm, so the header must not
// get to choose one. RS256 is required before any key is fetched, which is what
// refuses a token signed with the public key as an HMAC secret.
#[tokio::test]
async fn only_rs256_reaches_key_selection() {
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(jwks))).await;

    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("rsa01".into());
    let symmetric = encode(
        &header,
        &token_claims(&issuer, "flanforged"),
        &EncodingKey::from_secret(RSA_MODULUS.as_bytes()),
    )
    .unwrap_or_else(|error| unreachable!("token: {error}"));
    assert_eq!(
        verifier.verify(&symmetric).await,
        Err(AuthError::InvalidToken(TokenRejection::Algorithm))
    );

    // `alg: none` never becomes an algorithm at all, so it is refused a step
    // earlier, while the header is still being parsed.
    let unsigned = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJyZXBvOm93bmVyL3Byb2plY3QifQ.";
    assert_eq!(
        verifier.verify(unsigned).await,
        Err(AuthError::InvalidToken(TokenRejection::Header))
    );

    // And it cannot even be presented: an empty signature segment is not a
    // compact JWT.
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {unsigned}"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}")),
    );
    assert_eq!(
        bearer_token(&headers),
        Err(AuthError::InvalidToken(TokenRejection::CompactShape))
    );

    // `verify` is reachable without `bearer_token`, so it repeats the shape
    // check rather than trusting its caller.
    assert_eq!(
        verifier.verify("one two.three.four").await,
        Err(AuthError::InvalidToken(TokenRejection::CompactShape))
    );
}

// Key selection is the step an identity provider can weaken by accident. Each
// of these key sets is well-formed JSON that must still select nothing.
#[tokio::test]
async fn key_selection_refuses_an_ambiguous_or_unusable_key() {
    // Two keys under one id: the signer followed whichever the provider meant,
    // and verification cannot guess.
    let (verifier, issuer) =
        verifier_serving(json!({"keys": [rsa_jwk("rsa01"), rsa_jwk("rsa01")]})).await;
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::InvalidToken(TokenRejection::AmbiguousKey))
    );

    // A key that is not an RS256 signing key, five ways. Each publishes the
    // fixture key under the expected id, so only the disqualifier differs.
    let encryption = json!({
        "kty": "RSA", "n": RSA_MODULUS, "e": "AQAB",
        "kid": "rsa01", "alg": "RS256", "use": "enc"
    });
    let wrong_algorithm = json!({
        "kty": "RSA", "n": RSA_MODULUS, "e": "AQAB",
        "kid": "rsa01", "alg": "RS512", "use": "sig"
    });
    let no_algorithm = json!({
        "kty": "RSA", "n": RSA_MODULUS, "e": "AQAB", "kid": "rsa01", "use": "sig"
    });
    let cannot_verify = json!({
        "kty": "RSA", "n": RSA_MODULUS, "e": "AQAB", "kid": "rsa01",
        "alg": "RS256", "use": "sig", "key_ops": ["encrypt"]
    });
    // An elliptic-curve key published under the expected id, claiming RS256.
    let elliptic = json!({
        "kty": "EC", "crv": "P-256",
        "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
        "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        "kid": "rsa01", "alg": "RS256", "use": "sig"
    });
    for unusable in [
        encryption,
        wrong_algorithm,
        no_algorithm,
        cannot_verify,
        elliptic,
    ] {
        let label = unusable.to_string();
        let (verifier, issuer) = verifier_serving(json!({ "keys": [unusable] })).await;
        assert_eq!(
            verifier.verify(&signed_token(&issuer, "flanforged")).await,
            Err(AuthError::InvalidToken(TokenRejection::KeyUnusable)),
            "{label}"
        );
    }

    // An empty key id selects nothing, so it is refused rather than matched
    // against whatever key also carries none.
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(jwks))).await;
    assert_eq!(
        verifier
            .verify(&sign(&token_claims(&issuer, "flanforged"), ""))
            .await,
        Err(AuthError::InvalidToken(TokenRejection::KeyId))
    );
}

// A provider answering with nothing usable is unavailable, not a token problem:
// the distinction is what tells an operator where to look, and an empty key set
// must never be cached as an answer.
#[tokio::test]
async fn an_unusable_provider_response_is_reported_as_unavailable() {
    let (verifier, issuer) = verifier_serving(json!({ "keys": [] })).await;
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::Unavailable(ProviderFailure::EmptyKeySet))
    );

    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(not_json))).await;
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::Unavailable(ProviderFailure::Malformed))
    );

    // The body is measured while it streams, so an endless key set stops rather
    // than being buffered.
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(oversized_jwks))).await;
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::Unavailable(ProviderFailure::Oversize))
    );

    // Nothing listening at all is a fourth, separately reported problem.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    drop(listener);
    let issuer = format!("http://{address}/api/actions");
    let verifier = verifier_at(&issuer, &format!("http://{address}/jwks"));
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::Unavailable(ProviderFailure::Connect))
    );
}

// Rotation is why key selection retries with a forced refresh. Without the
// second attempt every token signed by a newly published key is refused, and no
// unknown-key test notices, because they all end in the same rejection anyway.
#[tokio::test]
async fn a_rotated_signing_key_is_picked_up_by_the_forced_refresh() {
    let rotated = Arc::new(AtomicBool::new(false));
    let (verifier, issuer) = verifier_for(
        Router::new()
            .route("/jwks", get(rotating_jwks))
            .with_state(rotated.clone()),
    )
    .await;
    verifier
        .verify(&signed_token(&issuer, "flanforged"))
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));

    // Age the cache past the forced-refresh floor and the cooldown but leave it
    // inside its lifetime, so an unforced read still returns the old key set.
    rotated.store(true, Ordering::SeqCst);
    verifier
        .age_cache_for_tests(Duration::from_mins(1), Duration::from_mins(1))
        .await;
    let claims = verifier
        .verify(&signed_token_with_kid(&issuer, "flanforged", "rsa02"))
        .await
        .unwrap_or_else(|error| unreachable!("rotated verify: {error}"));
    assert_eq!(claims.repository, "owner/project");

    // The refresh replaced the key set rather than adding to it, so the retired
    // key stops verifying at once.
    assert_eq!(
        verifier.verify(&signed_token(&issuer, "flanforged")).await,
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    );
}

// A correct signature over the wrong payload is still the wrong payload. These
// bodies are signed by the real key, so the claim set's own totality is the
// only thing between them and an authorized allocation.
#[tokio::test]
async fn a_signed_payload_still_has_to_carry_every_required_claim() {
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(jwks))).await;
    let expected = token_claims(&issuer, "flanforged");
    let complete =
        serde_json::to_value(&expected).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert_eq!(
        verifier.verify(&sign_payload(&complete, "rsa01")).await,
        Ok(expected)
    );

    // Every claim `Validation` requires is also a field of the claim set, which
    // is deserialized first — so a missing one is refused as an unreadable
    // payload, and `MissingClaim` stays defence in depth behind it.
    for claim in ["aud", "exp", "iat", "iss", "nbf", "sub"] {
        let mut partial = complete.clone();
        partial
            .as_object_mut()
            .unwrap_or_else(|| unreachable!("fixture is an object"))
            .remove(claim);
        assert_eq!(
            verifier.verify(&sign_payload(&partial, "rsa01")).await,
            Err(AuthError::InvalidToken(TokenRejection::Payload)),
            "{claim}"
        );
    }

    // A payload that is not this daemon's claim set at all, including one that
    // retypes a claim the policy later parses, fails before any policy check.
    let mut retyped = complete.clone();
    retyped["run_id"] = json!(42);
    for payload in [json!([1, 2, 3]), json!("a string"), retyped] {
        assert_eq!(
            verifier.verify(&sign_payload(&payload, "rsa01")).await,
            Err(AuthError::InvalidToken(TokenRejection::Payload)),
            "{payload}"
        );
    }
}

// The lifetime cap has two halves and only the upper one was asserted. A token
// that expires no later than it was issued is not short-lived, it is nonsense.
#[tokio::test]
async fn a_token_must_expire_after_it_was_issued_and_within_the_cap() {
    let (verifier, issuer) = verifier_for(Router::new().route("/jwks", get(jwks))).await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| unreachable!("clock: {error}"))
        .as_secs();

    let mut instant = token_claims(&issuer, "flanforged");
    instant.iat = now;
    instant.nbf = now.saturating_sub(1);
    instant.exp = now;
    assert_eq!(
        verifier.verify(&sign(&instant, "rsa01")).await,
        Err(AuthError::InvalidToken(TokenRejection::LifetimeCap))
    );

    // The cap itself is inclusive, so the longest allowed token still verifies.
    let mut longest = token_claims(&issuer, "flanforged");
    longest.iat = now;
    longest.nbf = now.saturating_sub(1);
    longest.exp = now + 7_200;
    verifier
        .verify(&sign(&longest, "rsa01"))
        .await
        .unwrap_or_else(|error| unreachable!("verify: {error}"));
}
